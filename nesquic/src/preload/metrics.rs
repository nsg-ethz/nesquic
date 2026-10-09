//! In-memory aggregation of everything the hooks observe.
//!
//! The hooks sit on the packet hot path of the benchmarked library, so the
//! state is lock-free atomics, except for the client's request streams
//! ([`Requests`]); the totals are only read once, at exit.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

use super::frame::{StreamFrame, Summary};
use super::MAX_CID_LEN;

/// The I/O calls hooked in [`super::io`], in reporting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Syscall {
    Write,
    Writev,
    Send,
    Sendto,
    Sendmsg,
    Sendmmsg,
    Read,
    Readv,
    Recv,
    Recvfrom,
    Recvmsg,
    Recvmmsg,
}

impl Syscall {
    const ALL: [Syscall; 12] = [
        Syscall::Write,
        Syscall::Writev,
        Syscall::Send,
        Syscall::Sendto,
        Syscall::Sendmsg,
        Syscall::Sendmmsg,
        Syscall::Read,
        Syscall::Readv,
        Syscall::Recv,
        Syscall::Recvfrom,
        Syscall::Recvmsg,
        Syscall::Recvmmsg,
    ];

    fn name(self) -> &'static str {
        match self {
            Syscall::Write => "write",
            Syscall::Writev => "writev",
            Syscall::Send => "send",
            Syscall::Sendto => "sendto",
            Syscall::Sendmsg => "sendmsg",
            Syscall::Sendmmsg => "sendmmsg",
            Syscall::Read => "read",
            Syscall::Readv => "readv",
            Syscall::Recv => "recv",
            Syscall::Recvfrom => "recvfrom",
            Syscall::Recvmsg => "recvmsg",
            Syscall::Recvmmsg => "recvmmsg",
        }
    }

    fn is_rx(self) -> bool {
        matches!(
            self,
            Syscall::Read
                | Syscall::Readv
                | Syscall::Recv
                | Syscall::Recvfrom
                | Syscall::Recvmsg
                | Syscall::Recvmmsg
        )
    }
}

struct IoCounter {
    count: AtomicU64,
    bytes: AtomicU64,
}

/// A monotonic `[first, last]` interval in nanoseconds; 0 means unset.
struct Window {
    first: AtomicU64,
    last: AtomicU64,
}

impl Window {
    const fn new() -> Self {
        Window {
            first: AtomicU64::new(0),
            last: AtomicU64::new(0),
        }
    }

    fn record(&self, now: u64) {
        set_once(&self.first, now);
        self.last.fetch_max(now, Relaxed);
    }

    fn seconds(&self) -> Option<f64> {
        let (first, last) = (self.first.load(Relaxed), self.last.load(Relaxed));
        (first != 0 && last > first).then(|| (last - first) as f64 / 1e9)
    }
}

/// Stores `now` in a timestamp that is still unset (0).
fn set_once(slot: &AtomicU64, now: u64) {
    if slot.load(Relaxed) == 0 {
        let _ = slot.compare_exchange(0, now, Relaxed, Relaxed);
    }
}

/// Nanoseconds from `start` to `end`, if both are set and ordered.
fn elapsed_ns(start: u64, end: u64) -> Option<u64> {
    (start != 0 && end >= start).then(|| end - start)
}

/// A stream of the connection that a packet's destination connection ID
/// stands for. Sent and received packets carry different IDs.
#[derive(PartialEq, Eq, Hash)]
struct StreamKey {
    cid: [u8; MAX_CID_LEN],
    cid_len: usize,
    stream: u64,
}

impl StreamKey {
    fn new(dcid: &[u8], stream: u64) -> Option<Self> {
        let mut cid = [0; MAX_CID_LEN];
        cid.get_mut(..dcid.len())?.copy_from_slice(dcid);
        Some(StreamKey {
            cid,
            cid_len: dcid.len(),
            stream,
        })
    }
}

/// The request streams of a client: every stream is one request, from the
/// first packet with its data to the last packet of its response.
///
/// A response cannot be tied to its request's connection, as the two
/// directions use different connection IDs, so responses on a stream ID
/// are paired with its requests in order. That keeps the mean exact.
///
/// ponytail: an entry of about 100 bytes per request is kept for the whole
/// run, i.e. 100 MB per 10^6 requests, and a client without a connection ID
/// of its own only counts each stream ID once across its connections.
/// Upgrade path: learn both connection IDs from the handshake packets and
/// key a single map by connection.
#[derive(Default)]
struct Requests {
    /// Streams whose request was seen, to ignore its retransmissions.
    started: HashSet<StreamKey>,
    /// Start of the unanswered requests, per stream ID.
    pending: HashMap<u64, VecDeque<u64>>,
    /// Start and end of the answered requests.
    finished: HashMap<StreamKey, (u64, u64)>,
}

impl Requests {
    fn record(&mut self, sent: bool, key: StreamKey, frame: &StreamFrame, now: u64) {
        if sent {
            if frame.offset == 0 && self.started.insert(key) {
                self.pending.entry(frame.id).or_default().push_back(now);
            }
            return;
        }

        // Data that was lost or reordered arrives after the FIN.
        if let Some((_, end)) = self.finished.get_mut(&key) {
            *end = now;
            return;
        }
        if !frame.fin {
            return;
        }
        let Some(starts) = self.pending.get_mut(&frame.id) else {
            return;
        };
        let Some(start) = starts.pop_front() else {
            return;
        };
        if starts.is_empty() {
            self.pending.remove(&frame.id);
        }
        self.finished.insert(key, (start, now));
    }
}

pub(crate) struct Metrics {
    io: [IoCounter; Syscall::ALL.len()],
    rx_bytes: AtomicU64,
    rx_window: Window,
    /// First UDP datagram sent: the start of the connection.
    tx_first: AtomicU64,
    /// First opened packet carrying stream data: the first response byte.
    response_first: AtomicU64,
    /// `None` unless [`Metrics::track_requests`] was called.
    requests: Mutex<Option<Requests>>,
    /// Nanoseconds spent in the hooked I/O calls on UDP sockets.
    io_ns: AtomicU64,
    /// Nanoseconds spent in the AEAD calls that protect packets.
    crypto_ns: AtomicU64,
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub acks_sent: AtomicU64,
    pub acks_received: AtomicU64,
}

pub(crate) static METRICS: Metrics = Metrics::new();

/// Runs `f`, adding the nanoseconds it takes to `total`, if any.
fn time<R>(total: Option<&AtomicU64>, f: impl FnOnce() -> R) -> R {
    let Some(total) = total else {
        return f();
    };
    let start = now_ns();
    let ret = f();
    total.fetch_add(now_ns() - start, Relaxed);
    ret
}

pub(crate) fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out-pointer; CLOCK_MONOTONIC is always available.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    // +1 keeps 0 free to mean "unset" in `Window`.
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64 + 1
}

impl Metrics {
    const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZERO: IoCounter = IoCounter {
            count: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        };
        Metrics {
            io: [ZERO; Syscall::ALL.len()],
            rx_bytes: AtomicU64::new(0),
            rx_window: Window::new(),
            tx_first: AtomicU64::new(0),
            response_first: AtomicU64::new(0),
            requests: Mutex::new(None),
            io_ns: AtomicU64::new(0),
            crypto_ns: AtomicU64::new(0),
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            acks_sent: AtomicU64::new(0),
            acks_received: AtomicU64::new(0),
        }
    }

    /// Runs the I/O call `f` and, if it is one on a UDP socket (`udp`),
    /// adds the time it takes to the I/O duration.
    pub fn time_io<R>(&self, udp: bool, f: impl FnOnce() -> R) -> R {
        time(udp.then_some(&self.io_ns), f)
    }

    /// Runs the AEAD call `f` and adds the time it takes to the crypto
    /// duration.
    pub fn time_crypto<R>(&self, f: impl FnOnce() -> R) -> R {
        time(super::enabled().then_some(&self.crypto_ns), f)
    }

    /// Adds `ns` to the crypto duration; for calls that turn out to be part
    /// of a packet's AEAD operation only once they are done.
    pub fn add_crypto(&self, ns: u64) {
        self.crypto_ns.fetch_add(ns, Relaxed);
    }

    /// Records one call of `syscall` on a UDP socket that moved `bytes`
    /// bytes (0 for calls that failed, e.g. with `EAGAIN`).
    pub fn record_io(&self, syscall: Syscall, bytes: u64) {
        let counter = &self.io[syscall as usize];
        counter.count.fetch_add(1, Relaxed);
        if bytes == 0 {
            return;
        }
        counter.bytes.fetch_add(bytes, Relaxed);
        if syscall.is_rx() {
            self.rx_bytes.fetch_add(bytes, Relaxed);
            self.rx_window.record(now_ns());
        } else {
            set_once(&self.tx_first, now_ns());
        }
    }

    /// Records one packet sealed (`sent`) or opened by the library's crypto,
    /// given what its decrypted payload carries.
    pub fn record_packet(&self, sent: bool, frames: &Summary) {
        let (packets, acks) = if sent {
            (&self.packets_sent, &self.acks_sent)
        } else {
            (&self.packets_received, &self.acks_received)
        };
        packets.fetch_add(1, Relaxed);
        if frames.acks > 0 {
            acks.fetch_add(frames.acks, Relaxed);
        }

        if !sent && (frames.stream_bytes > 0 || frames.stream_fin) {
            set_once(&self.response_first, now_ns());
        }
    }

    /// Starts measuring the latency of every request; for clients.
    pub fn track_requests(&self) {
        if let Ok(mut requests) = self.requests.lock() {
            *requests = Some(Requests::default());
        }
    }

    /// Records a STREAM frame of a 1-RTT packet with destination connection
    /// ID `dcid`, `sent` or received.
    pub fn record_stream(&self, sent: bool, dcid: &[u8], frame: &StreamFrame) {
        let Ok(mut requests) = self.requests.lock() else {
            return;
        };
        let Some(requests) = requests.as_mut() else {
            return;
        };
        let Some(key) = StreamKey::new(dcid, frame.id) else {
            return;
        };
        requests.record(sent, key, frame, now_ns());
    }

    /// Time to first byte (ms): from the first datagram the client sent,
    /// i.e. including the handshake, to the first packet with response data.
    fn ttfb_ms(&self) -> Option<f64> {
        let ns = elapsed_ns(
            self.tx_first.load(Relaxed),
            self.response_first.load(Relaxed),
        )?;
        Some(ns as f64 / 1e6)
    }

    /// The number of answered requests, their mean latency (ms): from the
    /// first packet with a request's data to the last packet with data of
    /// its response (normally the one with the FIN), and the time (ms) from
    /// the first request to the last response.
    fn request_latency_ms(&self) -> Option<(usize, f64, f64)> {
        let requests = self.requests.lock().ok()?;
        let finished = &requests.as_ref()?.finished;
        if finished.is_empty() {
            return None;
        }
        let (mut ns, mut first, mut last) = (0, u64::MAX, 0);
        for (start, end) in finished.values() {
            ns += end - start;
            first = first.min(*start);
            last = last.max(*end);
        }
        Some((
            finished.len(),
            ns as f64 / 1e6 / finished.len() as f64,
            (last - first) as f64 / 1e6,
        ))
    }

    /// Receive throughput in the unit the IUTs historically reported
    /// (bytes / 10^6 / s, see `utils::perf::Stats`), measured over UDP
    /// payload bytes between the first and the last received datagram.
    fn throughput(&self) -> Option<f64> {
        let secs = self.rx_window.seconds()?;
        Some(self.rx_bytes.load(Relaxed) as f64 / 1e6 / secs)
    }

    /// Renders all metrics as InfluxDB line protocol.
    ///
    /// - `nesquic`: `throughput`, for clients only (the receiving side)
    /// - `nesquic_latency`: `ttfb_ms`, the mean `request_latency_ms` and the
    ///   `requests` it is the mean of, for clients whose library's crypto
    ///   was hooked (see [`super::crypto`]), the `request_window_ms` from
    ///   the first request to the last response; for clients and servers,
    ///   the total `crypto_duration_ms` and `io_duration_ms` of the process
    /// - `nesquic_io`: per-syscall `count` and `volume_kb_sum`
    /// - `nesquic_quic`: packets and ACK frames sent and received, if the
    ///   library's crypto was hooked (see [`super::crypto`])
    pub fn line_protocol(&self, tags: &BTreeMap<String, String>, timestamp_ns: u128) -> String {
        let tags = tag_str(tags);
        let mut lines = String::new();

        let mut latency = Vec::new();
        if tags.contains(",mode=client") {
            if let Some(throughput) = self.throughput() {
                let _ = writeln!(
                    lines,
                    "nesquic{tags} throughput={throughput} {timestamp_ns}"
                );
            }

            if let Some(ttfb) = self.ttfb_ms() {
                latency.push(format!("ttfb_ms={ttfb}"));
            }
            if let Some((requests, mean, window)) = self.request_latency_ms() {
                latency.push(format!("request_latency_ms={mean}"));
                latency.push(format!("requests={requests}i"));
                latency.push(format!("request_window_ms={window}"));
            }
        }
        for (name, ns) in [
            ("crypto_duration_ms", &self.crypto_ns),
            ("io_duration_ms", &self.io_ns),
        ] {
            let ns = ns.load(Relaxed);
            if ns > 0 {
                latency.push(format!("{name}={}", ns as f64 / 1e6));
            }
        }
        if !latency.is_empty() {
            let _ = writeln!(
                lines,
                "nesquic_latency{tags} {} {timestamp_ns}",
                latency.join(",")
            );
        }

        for syscall in Syscall::ALL {
            let counter = &self.io[syscall as usize];
            let count = counter.count.load(Relaxed);
            if count == 0 {
                continue;
            }
            let kb = counter.bytes.load(Relaxed) as f64 / 1000.0;
            let _ = writeln!(
                lines,
                "nesquic_io{tags},syscall={} volume_kb_sum={kb},count={count}i {timestamp_ns}",
                syscall.name()
            );
        }

        let quic = [
            ("packets_sent", &self.packets_sent),
            ("packets_received", &self.packets_received),
            ("acks_sent", &self.acks_sent),
            ("acks_received", &self.acks_received),
        ]
        .map(|(name, value)| (name, value.load(Relaxed)));
        if quic.iter().any(|(_, value)| *value > 0) {
            let fields: Vec<String> = quic.iter().map(|(k, v)| format!("{k}={v}i")).collect();
            let _ = writeln!(
                lines,
                "nesquic_quic{tags} {} {timestamp_ns}",
                fields.join(",")
            );
        }

        lines
    }
}

fn escape_tag(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace('=', "\\=")
        .replace(' ', "\\ ")
}

/// Renders `tags` as the `,k=v,...` tag set of a line (empty values are
/// dropped, since InfluxDB rejects them).
fn tag_str(tags: &BTreeMap<String, String>) -> String {
    tags.iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!(",{}={}", escape_tag(k), escape_tag(v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn escapes_tags() {
        let t = tags(&[("job", "a b,c=d"), ("empty", ""), ("mode", "client")]);
        assert_eq!(tag_str(&t), ",job=a\\ b\\,c\\=d,mode=client");
    }

    #[test]
    fn renders_io_and_quic() {
        let m = Metrics::new();
        m.record_io(Syscall::Sendmsg, 1200);
        m.record_io(Syscall::Sendmsg, 1300);
        m.record_io(Syscall::Recvmmsg, 0);
        m.acks_sent.fetch_add(3, Relaxed);

        let out = m.line_protocol(&tags(&[("job", "j"), ("mode", "server")]), 42);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "nesquic_io,job=j,mode=server,syscall=sendmsg volume_kb_sum=2.5,count=2i 42",
                "nesquic_io,job=j,mode=server,syscall=recvmmsg volume_kb_sum=0,count=1i 42",
                "nesquic_quic,job=j,mode=server packets_sent=0i,packets_received=0i,acks_sent=3i,acks_received=0i 42",
            ]
        );
    }

    #[test]
    fn throughput_only_for_clients() {
        let m = Metrics::new();
        m.rx_bytes.store(2_000_000, Relaxed);
        m.rx_window.first.store(1, Relaxed);
        m.rx_window.last.store(1_000_000_001, Relaxed);
        assert_eq!(m.throughput(), Some(2.0));

        let client = m.line_protocol(&tags(&[("mode", "client")]), 1);
        assert!(client.starts_with("nesquic,mode=client throughput=2 1\n"));
        let server = m.line_protocol(&tags(&[("mode", "server")]), 1);
        assert!(!server.contains("throughput"));
    }

    fn stream(bytes: u64, fin: bool) -> Summary {
        Summary {
            acks: 0,
            stream_bytes: bytes,
            stream_fin: fin,
        }
    }

    #[test]
    fn records_packets_and_acks() {
        let m = Metrics::new();
        m.record_packet(
            true,
            &Summary {
                acks: 2,
                ..Default::default()
            },
        );
        m.record_packet(false, &Summary::default());
        assert_eq!(m.packets_sent.load(Relaxed), 1);
        assert_eq!(m.acks_sent.load(Relaxed), 2);
        assert_eq!(m.packets_received.load(Relaxed), 1);
        // Handshake and ACK-only packets say nothing about the response.
        assert_eq!(m.response_first.load(Relaxed), 0);
    }

    #[test]
    fn ttfb_timestamps() {
        let m = Metrics::new();
        m.record_io(Syscall::Sendmsg, 1200);
        m.record_packet(true, &stream(8, true));
        assert_eq!(m.response_first.load(Relaxed), 0);
        m.record_packet(false, &stream(1000, false));

        let (tx, first) = (m.tx_first.load(Relaxed), m.response_first.load(Relaxed));
        assert!(tx != 0 && tx <= first);
        // Only the first response packet counts.
        m.record_packet(false, &stream(0, true));
        assert_eq!(m.response_first.load(Relaxed), first);
    }

    fn frame(id: u64, offset: u64, fin: bool) -> StreamFrame {
        StreamFrame {
            id,
            offset,
            len: 8,
            fin,
        }
    }

    fn key(dcid: &[u8], stream: u64) -> StreamKey {
        StreamKey::new(dcid, stream).unwrap()
    }

    #[test]
    fn measures_every_request() {
        let (server, client) = (&[1, 2][..], &[3][..]);
        let mut r = Requests::default();

        r.record(true, key(server, 0), &frame(0, 0, true), 100);
        r.record(true, key(server, 4), &frame(4, 0, true), 200);
        // A retransmitted request does not restart its clock.
        r.record(true, key(server, 0), &frame(0, 0, true), 250);
        r.record(false, key(client, 4), &frame(4, 0, false), 300);
        assert!(r.finished.is_empty());
        r.record(false, key(client, 4), &frame(4, 8, true), 500);
        r.record(false, key(client, 0), &frame(0, 0, true), 600);
        // Data that arrives after the FIN still belongs to the response.
        r.record(false, key(client, 0), &frame(0, 0, false), 700);
        // A response without a request is ignored.
        r.record(false, key(client, 8), &frame(8, 0, true), 800);

        assert_eq!(r.finished.get(&key(client, 4)), Some(&(200, 500)));
        assert_eq!(r.finished.get(&key(client, 0)), Some(&(100, 700)));
        assert_eq!(r.finished.len(), 2);
        assert!(r.pending.is_empty());
    }

    #[test]
    fn pairs_requests_of_several_connections() {
        let mut r = Requests::default();
        r.record(true, key(&[1], 0), &frame(0, 0, true), 100);
        r.record(true, key(&[2], 0), &frame(0, 0, true), 200);
        r.record(false, key(&[4], 0), &frame(0, 0, true), 400);
        r.record(false, key(&[3], 0), &frame(0, 0, true), 700);

        let total: u64 = r.finished.values().map(|(start, end)| end - start).sum();
        assert_eq!((r.finished.len(), total), (2, 800));
    }

    #[test]
    fn renders_latency_for_clients_only() {
        let m = Metrics::new();
        m.tx_first.store(1_000_001, Relaxed);
        m.response_first.store(4_000_001, Relaxed);
        assert_eq!(m.ttfb_ms(), Some(3.0));
        // Servers do not track requests.
        m.record_stream(true, &[1], &frame(0, 0, true));
        assert_eq!(m.request_latency_ms(), None);

        m.track_requests();
        if let Some(requests) = m.requests.lock().unwrap().as_mut() {
            requests.finished.insert(key(&[1], 0), (1_000_000, 6_000_000));
            requests.finished.insert(key(&[1], 4), (1_000_000, 10_000_000));
        }
        assert_eq!(m.request_latency_ms(), Some((2, 7.0, 9.0)));

        m.add_crypto(2_500_000);
        m.time_io(true, || m.io_ns.fetch_add(500_000, Relaxed));
        let io = m.io_ns.load(Relaxed) as f64 / 1e6;
        assert!((0.5..0.6).contains(&io));
        m.time_io(false, || ());
        assert_eq!(m.io_ns.load(Relaxed) as f64 / 1e6, io);

        let client = m.line_protocol(&tags(&[("mode", "client")]), 1);
        assert!(client.contains(&format!(
            "nesquic_latency,mode=client ttfb_ms=3,request_latency_ms=7,requests=2i,request_window_ms=9,\
             crypto_duration_ms=2.5,io_duration_ms={io} 1\n"
        )));
        // Servers only report their durations.
        let server = m.line_protocol(&tags(&[("mode", "server")]), 1);
        assert!(server.contains(&format!(
            "nesquic_latency,mode=server crypto_duration_ms=2.5,io_duration_ms={io} 1\n"
        )));
    }

    #[test]
    fn no_latency_without_crypto_hooks() {
        let m = Metrics::new();
        m.track_requests();
        m.record_io(Syscall::Sendmsg, 1200);
        m.record_io(Syscall::Recvmsg, 1200);
        assert_eq!(m.ttfb_ms(), None);
        assert_eq!(m.request_latency_ms(), None);
        let client = m.line_protocol(&tags(&[("mode", "client")]), 1);
        assert!(!client.contains("nesquic_latency"));
    }

    #[test]
    fn empty_when_nothing_observed() {
        assert!(Metrics::new().line_protocol(&BTreeMap::new(), 1).is_empty());
    }
}
