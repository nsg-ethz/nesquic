//! In-memory aggregation of everything the hooks observe.
//!
//! The hooks sit on the packet hot path of the benchmarked library, so all
//! state is lock-free atomics; the totals are only read once, at exit.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

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
        if self.first.load(Relaxed) == 0 {
            let _ = self.first.compare_exchange(0, now, Relaxed, Relaxed);
        }
        self.last.fetch_max(now, Relaxed);
    }

    fn seconds(&self) -> Option<f64> {
        let (first, last) = (self.first.load(Relaxed), self.last.load(Relaxed));
        (first != 0 && last > first).then(|| (last - first) as f64 / 1e9)
    }
}

pub(crate) struct Metrics {
    io: [IoCounter; Syscall::ALL.len()],
    rx_bytes: AtomicU64,
    rx_window: Window,
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub acks_sent: AtomicU64,
    pub acks_received: AtomicU64,
}

pub(crate) static METRICS: Metrics = Metrics::new();

fn now_ns() -> u64 {
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
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            acks_sent: AtomicU64::new(0),
            acks_received: AtomicU64::new(0),
        }
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
        }
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
    /// - `nesquic_io`: per-syscall `count` and `volume_kb_sum`
    /// - `nesquic_quic`: packets and ACK frames sent and received, if the
    ///   library's crypto was hooked (see [`super::crypto`])
    pub fn line_protocol(&self, tags: &BTreeMap<String, String>, timestamp_ns: u128) -> String {
        let tags = tag_str(tags);
        let mut lines = String::new();

        if tags.contains(",mode=client") {
            if let Some(throughput) = self.throughput() {
                let _ = writeln!(
                    lines,
                    "nesquic{tags} throughput={throughput} {timestamp_ns}"
                );
            }
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

    #[test]
    fn empty_when_nothing_observed() {
        assert!(Metrics::new().line_protocol(&BTreeMap::new(), 1).is_empty());
    }
}
