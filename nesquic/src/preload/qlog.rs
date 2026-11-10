//! Emits a qlog trace of the QUIC packets observed in the crypto hooks.
//!
//! Enabled by setting `NQ_QLOG` to an output path; if unset, no qlog file
//! is written. This is meant for debugging only: every packet goes through a
//! global lock, a second frame walk and a serializer, which slows down the
//! monitored library.
//!
//! Besides `packet_sent` and `packet_received` with all their frames, the
//! trace holds what an observer of the decrypted packets can infer, per
//! connection:
//!   - `parameters_set`: the cipher suite and both sides' transport
//!     parameters, from the TLS handshake in the CRYPTO frames ([`tls`]);
//!   - `metrics_updated`: the RTT estimates of RFC 9002, from the received
//!     ACK frames and the send times of the packets they acknowledge, and
//!     the bytes and packets in flight (sent ack-eliciting packets that are
//!     neither acknowledged nor lost);
//!   - `packet_lost`: sent packets that the packet and time thresholds of
//!     RFC 9002 declare lost when an ACK arrives. The library runs its own
//!     loss detection, which may decide differently;
//!   - `key_updated`: 1-RTT key updates, from the key phase bit.
//!
//! [`tls`]: super::tls

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use qlog::events::connectivity::TransportOwner;
use qlog::events::quic::{
    AckedRanges, ErrorSpace, MetricsUpdated, PacketHeader, PacketLost, PacketLostTrigger,
    PacketReceived, PacketSent, PacketType, QuicFrame, StreamType,
};
use qlog::events::security::{KeyType, KeyUpdated};
use qlog::events::{Event, EventData, EventImportance, Eventable, RawInfo};
use qlog::streamer::QlogStreamer;
use qlog::{Configuration, Token, TraceSeq, VantagePoint, VantagePointType, QLOG_VERSION};
use s2n_codec::DecoderBuffer;
use s2n_quic_core::frame::ack::AckRangesDecoder;
use s2n_quic_core::frame::ack_elicitation::AckElicitable;
use s2n_quic_core::frame::{Ack, FrameRef};
use s2n_quic_core::packet::number::{PacketNumber, PacketNumberSpace};
use s2n_quic_core::recovery::{RttEstimator, DEFAULT_INITIAL_RTT};
use s2n_quic_core::stream;
use s2n_quic_core::time::Timestamp;
use s2n_quic_core::transport::parameters::MaxAckDelay;
use s2n_quic_core::varint::VarInt;
use serde::ser::{Error as _, Serialize, Serializer};
use serde_json::{Map, Value};

use super::frame::{self, Frame};
use super::tls::{CryptoStream, Parameters};
use super::{context, hex, parse_quic_header, QuicHeader, QuicPacketType};

/// The tag length of every QUIC version 1 AEAD (AES-GCM, ChaCha20-Poly1305).
const TAG_LEN: usize = 16;
/// The ACK delay exponent without a transport parameter (RFC 9000 18.2).
const DEFAULT_ACK_DELAY_EXPONENT: u8 = 3;
/// kPacketThreshold of RFC 9002 section 6.1.1.
const PACKET_THRESHOLD: u64 = 3;
/// Received datagram buffers and unsent packets kept at most.
const MAX_BUFFERS: usize = 1 << 14;
/// Ciphertext bytes that find a packet's copy in a datagram: enough to rule
/// out chance matches, not more than the shortest packets have.
const MIN_CIPHERTEXT: usize = 4;
const MAX_CIPHERTEXT: usize = 16;

const SPACES: [PacketNumberSpace; 3] = [
    PacketNumberSpace::Initial,
    PacketNumberSpace::Handshake,
    PacketNumberSpace::ApplicationData,
];

/// Event data, with fields that qlog 0.15 lacks.
struct Extended {
    data: EventData,
    extra: Map<String, Value>,
}

impl From<EventData> for Extended {
    fn from(data: EventData) -> Self {
        Extended {
            data,
            extra: Map::new(),
        }
    }
}

/// An event with `extra` added to its `data` object.
struct WithExtraData {
    event: Event,
    extra: Map<String, Value>,
}

impl Serialize for WithExtraData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.extra.is_empty() {
            return self.event.serialize(serializer);
        }
        let mut value = serde_json::to_value(&self.event).map_err(S::Error::custom)?;
        if let Some(data) = value.get_mut("data").and_then(Value::as_object_mut) {
            data.extend(self.extra.clone());
        }
        value.serialize(serializer)
    }
}

impl Eventable for WithExtraData {
    fn importance(&self) -> EventImportance {
        self.event.importance()
    }

    fn set_time(&mut self, time: f32) {
        self.event.set_time(time);
    }
}

static QLOG: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();

/// A packet sent and not yet acknowledged or declared lost.
struct Sent {
    at: Instant,
    size: u64,
    ack_eliciting: bool,
}

/// What is known about one connection. Arrays indexed by direction hold the
/// received packets' state at 0 and the sent packets' state at 1.
struct Connection {
    group_id: String,
    largest: [[Option<PacketNumber>; 3]; 2],
    unacked: [BTreeMap<u64, Sent>; 3],
    bytes_in_flight: u64,
    packets_in_flight: u64,
    rtt: RttEstimator,
    handshake_confirmed: bool,
    /// The ACK delay exponent of the ACK frames in each direction, from the
    /// transport parameters sent in the same direction.
    ack_delay_exponent: [u8; 2],
    key_phase: [Option<bool>; 2],
    key_generation: [u32; 2],
    /// The CRYPTO streams of the Initial and Handshake spaces.
    crypto: [[CryptoStream; 2]; 2],
}

struct Trace {
    streamer: QlogStreamer,
    start: Instant,
    client: bool,
    connections: Vec<Connection>,
    /// The connection of each of the monitored process's own connection
    /// IDs (destination of received packets) and of its peers' (destination
    /// of sent packets).
    local_cids: HashMap<Vec<u8>, usize>,
    remote_cids: HashMap<Vec<u8>, usize>,
    /// The connections whose own (local) or peer (remote) connection IDs
    /// are zero-length.
    zero_len_local: Vec<usize>,
    zero_len_remote: Vec<usize>,
    /// The connection of each UDP 4-tuple, which tells apart connections
    /// with zero-length connection IDs.
    tuples: HashMap<Tuple, usize>,
    /// The buffers of the latest received datagrams, by start address: the
    /// end address and the 4-tuple. Libraries open packets where they
    /// received them.
    received: BTreeMap<usize, (usize, Tuple)>,
    /// Sent packets until the datagram that carries them is sent.
    unsent: VecDeque<Pending>,
}

/// A sealed packet and how to find the datagram that carries it: libraries
/// send the buffer they sealed it into (`wire`), or a copy, which starts
/// with the ciphertext after the header.
struct Pending {
    wire: usize,
    header_len: usize,
    /// The first bytes of the ciphertext.
    ciphertext: Vec<u8>,
    unsent: Unsent,
}

impl Pending {
    fn in_datagram(&self, datagram: &[u8]) -> bool {
        let start = datagram.as_ptr() as usize;
        (start..start + datagram.len()).contains(&self.wire)
            || self.ciphertext.len() >= MIN_CIPHERTEXT
                && datagram.get(self.header_len..self.header_len + self.ciphertext.len())
                    == Some(&self.ciphertext[..])
    }
}

/// A UDP socket and the peer it sends to or receives from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Tuple {
    fd: i32,
    peer: Option<SocketAddr>,
}

/// A sealed packet before its datagram is sent.
enum Unsent {
    /// Of a known connection, whose 4-tuple the datagram tells.
    Known(usize),
    /// Of a connection that only the datagram's 4-tuple tells.
    Deferred {
        ad: Vec<u8>,
        payload: Vec<u8>,
        at: Instant,
    },
}

fn init_trace() -> Option<Mutex<Trace>> {
    let path = std::env::var("NQ_QLOG").ok()?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;

    let client = context::Context::current().mode.as_deref() == Some("client");
    let trace = TraceSeq::new(
        VantagePoint {
            name: None,
            ty: if client {
                VantagePointType::Client
            } else {
                VantagePointType::Server
            },
            flow: None,
        },
        Some("nesquic".to_string()),
        Some("QUIC packets observed via crypto hooks".to_string()),
        Some(Configuration {
            time_offset: Some(0.0),
            original_uris: None,
        }),
        None,
    );

    let start = Instant::now();
    let mut streamer = QlogStreamer::new(
        QLOG_VERSION.to_string(),
        Some("nesquic".to_string()),
        None,
        None,
        start,
        trace,
        EventImportance::Extra,
        Box::new(file),
    );

    streamer.start_log().ok()?;
    Some(Mutex::new(Trace {
        streamer,
        start,
        client,
        connections: Vec::new(),
        local_cids: HashMap::new(),
        remote_cids: HashMap::new(),
        zero_len_local: Vec::new(),
        zero_len_remote: Vec::new(),
        tuples: HashMap::new(),
        received: BTreeMap::new(),
        unsent: VecDeque::new(),
    }))
}

fn packet_type(ty: QuicPacketType) -> PacketType {
    match ty {
        QuicPacketType::Initial => PacketType::Initial,
        QuicPacketType::ZeroRtt => PacketType::ZeroRtt,
        QuicPacketType::Handshake => PacketType::Handshake,
        QuicPacketType::Retry => PacketType::Retry,
        QuicPacketType::VersionNegotiation => PacketType::VersionNegotiation,
        QuicPacketType::Short => PacketType::OneRtt,
    }
}

/// The index of the packet number space of `ty` in [`SPACES`].
fn space(ty: QuicPacketType) -> Option<usize> {
    match ty {
        QuicPacketType::Initial => Some(0),
        QuicPacketType::Handshake => Some(1),
        QuicPacketType::ZeroRtt | QuicPacketType::Short => Some(2),
        QuicPacketType::Retry | QuicPacketType::VersionNegotiation => None,
    }
}

fn ms(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1000.0
}

fn ack_delay(ack: &Ack<AckRangesDecoder>, exponent: u8) -> Duration {
    Duration::from_micros(ack.ack_delay.as_u64().saturating_mul(1 << exponent))
}

fn stream_type(ty: stream::StreamType) -> StreamType {
    match ty {
        stream::StreamType::Bidirectional => StreamType::Bidirectional,
        stream::StreamType::Unidirectional => StreamType::Unidirectional,
    }
}

/// Converts `frame` to its qlog form; ACK delays are scaled by `exponent`.
fn qlog_frame(frame: &Frame, exponent: u8) -> QuicFrame {
    let frame = match frame {
        Frame::Quic(frame) => frame,
        Frame::Extension(ty) => {
            return QuicFrame::Unknown {
                raw_frame_type: *ty,
                frame_type_value: None,
                raw: None,
            }
        }
    };
    let u32 = |v: VarInt| u32::try_from(v.as_u64()).unwrap_or(u32::MAX);
    match frame {
        FrameRef::Padding(f) => QuicFrame::Padding {
            length: None,
            payload_length: f.length as u32,
        },
        FrameRef::Ping(_) => QuicFrame::Ping {
            length: None,
            payload_length: None,
        },
        FrameRef::Ack(f) => QuicFrame::Ack {
            ack_delay: Some(ms(ack_delay(f, exponent))),
            acked_ranges: Some(AckedRanges::Double(
                f.ack_ranges()
                    .map(|r| (r.start().as_u64(), r.end().as_u64()))
                    .collect(),
            )),
            ect1: f.ecn_counts.map(|c| c.ect_1_count.as_u64()),
            ect0: f.ecn_counts.map(|c| c.ect_0_count.as_u64()),
            ce: f.ecn_counts.map(|c| c.ce_count.as_u64()),
            length: None,
            payload_length: None,
        },
        FrameRef::ResetStream(f) => QuicFrame::ResetStream {
            stream_id: f.stream_id.as_u64(),
            error_code: f.application_error_code.as_u64(),
            final_size: f.final_size.as_u64(),
            length: None,
            payload_length: None,
        },
        FrameRef::StopSending(f) => QuicFrame::StopSending {
            stream_id: f.stream_id.as_u64(),
            error_code: f.application_error_code.as_u64(),
            length: None,
            payload_length: None,
        },
        FrameRef::Crypto(f) => QuicFrame::Crypto {
            offset: f.offset.as_u64(),
            length: f.data.len() as u64,
        },
        FrameRef::NewToken(f) => QuicFrame::NewToken {
            token: Token {
                ty: None,
                details: None,
                raw: Some(RawInfo {
                    length: Some(f.token.len() as u64),
                    payload_length: None,
                    data: Some(hex(f.token)),
                }),
            },
        },
        FrameRef::Stream(f) => QuicFrame::Stream {
            stream_id: f.stream_id.as_u64(),
            offset: f.offset.as_u64(),
            length: f.data.len() as u64,
            fin: Some(f.is_fin),
            raw: None,
        },
        FrameRef::MaxData(f) => QuicFrame::MaxData {
            maximum: f.maximum_data.as_u64(),
        },
        FrameRef::MaxStreamData(f) => QuicFrame::MaxStreamData {
            stream_id: f.stream_id.as_u64(),
            maximum: f.maximum_stream_data.as_u64(),
        },
        FrameRef::MaxStreams(f) => QuicFrame::MaxStreams {
            stream_type: stream_type(f.stream_type),
            maximum: f.maximum_streams.as_u64(),
        },
        FrameRef::DataBlocked(f) => QuicFrame::DataBlocked {
            limit: f.data_limit.as_u64(),
        },
        FrameRef::StreamDataBlocked(f) => QuicFrame::StreamDataBlocked {
            stream_id: f.stream_id.as_u64(),
            limit: f.stream_data_limit.as_u64(),
        },
        FrameRef::StreamsBlocked(f) => QuicFrame::StreamsBlocked {
            stream_type: stream_type(f.stream_type),
            limit: f.stream_limit.as_u64(),
        },
        FrameRef::NewConnectionId(f) => QuicFrame::NewConnectionId {
            sequence_number: u32(f.sequence_number),
            retire_prior_to: u32(f.retire_prior_to),
            connection_id_length: Some(f.connection_id.len() as u8),
            connection_id: hex(f.connection_id),
            stateless_reset_token: Some(hex(f.stateless_reset_token)),
        },
        FrameRef::RetireConnectionId(f) => QuicFrame::RetireConnectionId {
            sequence_number: u32(f.sequence_number),
        },
        FrameRef::PathChallenge(f) => QuicFrame::PathChallenge {
            data: Some(hex(f.data)),
        },
        FrameRef::PathResponse(f) => QuicFrame::PathResponse {
            data: Some(hex(f.data)),
        },
        FrameRef::ConnectionClose(f) => QuicFrame::ConnectionClose {
            // Only the transport variant carries a frame type.
            error_space: Some(if f.frame_type.is_some() {
                ErrorSpace::TransportError
            } else {
                ErrorSpace::ApplicationError
            }),
            error_code: Some(f.error_code.as_u64()),
            error_code_value: None,
            reason: f.reason.map(|r| String::from_utf8_lossy(r).into_owned()),
            trigger_frame_type: f.frame_type.map(VarInt::as_u64),
        },
        FrameRef::HandshakeDone(_) => QuicFrame::HandshakeDone,
        FrameRef::Datagram(f) => QuicFrame::Datagram {
            length: f.data.len() as u64,
            raw: None,
        },
        // s2n-quic's own extension frames.
        FrameRef::DcStatelessResetTokens(_) | FrameRef::MtuProbingComplete(_) => {
            QuicFrame::Unknown {
                raw_frame_type: 0,
                frame_type_value: None,
                raw: None,
            }
        }
    }
}

fn ack_eliciting(frame: &Frame) -> bool {
    match frame {
        Frame::Quic(frame) => frame.ack_elicitation().is_ack_eliciting(),
        Frame::Extension(_) => true,
    }
}

impl Connection {
    fn new(group_id: String) -> Self {
        Connection {
            group_id,
            largest: Default::default(),
            unacked: Default::default(),
            bytes_in_flight: 0,
            packets_in_flight: 0,
            rtt: RttEstimator::new(DEFAULT_INITIAL_RTT),
            handshake_confirmed: false,
            ack_delay_exponent: [DEFAULT_ACK_DELAY_EXPONENT; 2],
            key_phase: [None; 2],
            key_generation: [0; 2],
            crypto: Default::default(),
        }
    }

    /// The full packet number of the packet with header `ad` in `space`
    /// (RFC 9000 appendix A.3).
    fn packet_number(&mut self, dir: usize, space: usize, ad: &[u8]) -> Option<u64> {
        let first = *ad.first()?;
        let len = SPACES[space].new_packet_number_len(first);
        let truncated = ad.get(ad.len().checked_sub(len.bytesize())?..)?;
        let (truncated, _) = len
            .decode_truncated_packet_number(DecoderBuffer::new(truncated))
            .ok()?;
        let largest = &mut self.largest[dir][space];
        let pn = truncated
            .expand(largest.unwrap_or_else(|| SPACES[space].new_packet_number(VarInt::from_u8(0))));
        *largest = Some(largest.map_or(pn, |l| l.max(pn)));
        Some(pn.as_u64())
    }

    fn on_sent(&mut self, space: usize, pn: u64, sent: Sent) {
        if sent.ack_eliciting {
            self.bytes_in_flight += sent.size;
            self.packets_in_flight += 1;
        }
        self.unacked[space].insert(pn, sent);
    }

    fn remove(&mut self, space: usize, pn: u64) -> Option<Sent> {
        let sent = self.unacked[space].remove(&pn)?;
        if sent.ack_eliciting {
            self.bytes_in_flight -= sent.size;
            self.packets_in_flight -= 1;
        }
        Some(sent)
    }

    /// Processes an ACK frame received in `space` at `now`, as RFC 9002
    /// sections 5 and 6.1 do, and returns the resulting events.
    fn on_ack(
        &mut self,
        space: usize,
        ack: &Ack<AckRangesDecoder>,
        now: Instant,
        timestamp: Timestamp,
    ) -> Vec<EventData> {
        let largest = ack.largest_acknowledged().as_u64();
        let acked: Vec<u64> = ack
            .ack_ranges()
            .flat_map(|r| {
                let range = r.start().as_u64()..=r.end().as_u64();
                self.unacked[space].range(range).map(|(pn, _)| *pn)
            })
            .collect();
        let newly_acked: Vec<(u64, Sent)> = acked
            .into_iter()
            .filter_map(|pn| Some((pn, self.remove(space, pn)?)))
            .collect();
        if newly_acked.is_empty() {
            return Vec::new();
        }

        let mut rtt_updated = false;
        if let Some((_, sent)) = newly_acked.iter().find(|(pn, _)| *pn == largest) {
            if newly_acked.iter().any(|(_, sent)| sent.ack_eliciting) {
                self.rtt.update_rtt(
                    ack_delay(ack, self.ack_delay_exponent[0]),
                    now - sent.at,
                    timestamp,
                    self.handshake_confirmed,
                    SPACES[space],
                );
                rtt_updated = true;
            }
        }

        let mut events = Vec::new();
        let threshold = self.rtt.loss_time_threshold();
        let lost: Vec<(u64, PacketLostTrigger)> = self.unacked[space]
            .range(..largest)
            .filter_map(|(pn, sent)| {
                if pn + PACKET_THRESHOLD <= largest {
                    Some((*pn, PacketLostTrigger::ReorderingThreshold))
                } else if now - sent.at >= threshold {
                    Some((*pn, PacketLostTrigger::TimeThreshold))
                } else {
                    None
                }
            })
            .collect();
        let ty = [
            QuicPacketType::Initial,
            QuicPacketType::Handshake,
            QuicPacketType::Short,
        ][space];
        for (pn, trigger) in lost {
            self.remove(space, pn);
            events.push(EventData::PacketLost(PacketLost {
                header: Some(PacketHeader::new(
                    packet_type(ty),
                    Some(pn),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )),
                frames: None,
                trigger: Some(trigger),
            }));
        }
        events.insert(0, self.metrics(rtt_updated));
        events
    }

    fn metrics(&self, rtt: bool) -> EventData {
        let rtt = |f: fn(&RttEstimator) -> Duration| rtt.then(|| ms(f(&self.rtt)));
        EventData::MetricsUpdated(MetricsUpdated {
            min_rtt: rtt(RttEstimator::min_rtt),
            smoothed_rtt: rtt(RttEstimator::smoothed_rtt),
            latest_rtt: rtt(RttEstimator::latest_rtt),
            rtt_variance: rtt(RttEstimator::rttvar),
            bytes_in_flight: Some(self.bytes_in_flight),
            packets_in_flight: Some(self.packets_in_flight),
            ..Default::default()
        })
    }
}

impl Trace {
    /// The connection of a packet with `header`, `sent` or received. Each
    /// side's connection IDs are learned from the long headers and the
    /// NEW_CONNECTION_ID frames. A packet with only zero-length IDs is of
    /// the connection of its 4-tuple (`by_tuple`), if known, else of the
    /// single connection that uses such IDs on that side; `None` if that is
    /// ambiguous.
    ///
    /// ponytail: connection IDs and 4-tuples are never forgotten. Upgrade
    /// path: retire IDs with RETIRE_CONNECTION_ID.
    fn connection(
        &mut self,
        header: &QuicHeader,
        sent: bool,
        by_tuple: Option<usize>,
    ) -> Option<usize> {
        let dcid = Some(&header.dcid[..]);
        let (local, remote) = if sent {
            (header.scid.as_deref(), dcid)
        } else {
            (dcid, header.scid.as_deref())
        };
        fn non_empty(cid: Option<&[u8]>) -> Option<&[u8]> {
            cid.filter(|cid| !cid.is_empty())
        }
        let known = non_empty(local)
            .and_then(|cid| self.local_cids.get(cid))
            .or_else(|| non_empty(remote).and_then(|cid| self.remote_cids.get(cid)))
            .copied();
        // An unknown destination connection ID starts a connection, unless
        // it is zero-length.
        let zero_len = if sent {
            &self.zero_len_remote
        } else {
            &self.zero_len_local
        };
        let id = match known {
            Some(id) => id,
            None if !header.dcid.is_empty() => self.new_connection(&header.dcid),
            None => match (by_tuple, &zero_len[..]) {
                (Some(id), _) | (None, &[id]) => id,
                (None, []) => self.new_connection(&header.dcid),
                (None, _) => return None,
            },
        };
        for (cids, zero_len, cid) in [
            (&mut self.local_cids, &mut self.zero_len_local, local),
            (&mut self.remote_cids, &mut self.zero_len_remote, remote),
        ] {
            match cid {
                Some([]) if !zero_len.contains(&id) => zero_len.push(id),
                Some([]) | None => {}
                Some(cid) => {
                    cids.entry(cid.to_vec()).or_insert(id);
                }
            }
        }
        Some(id)
    }

    fn new_connection(&mut self, dcid: &[u8]) -> usize {
        self.connections.push(Connection::new(hex(dcid)));
        self.connections.len() - 1
    }

    /// The connection of the 4-tuple `tuple`: the one it was seen with, or
    /// the only one seen on its socket (connected sockets pass no peer).
    fn tuple_connection(&self, tuple: Tuple) -> Option<usize> {
        if let Some(id) = self.tuples.get(&tuple) {
            return Some(*id);
        }
        let mut ids = self
            .tuples
            .iter()
            .filter(|(t, _)| t.fd == tuple.fd)
            .map(|(_, id)| *id);
        let id = ids.next()?;
        ids.all(|other| other == id).then_some(id)
    }

    /// The 4-tuple of the received datagram whose buffer holds `wire`.
    fn received_tuple(&self, wire: usize) -> Option<Tuple> {
        let (_, (end, tuple)) = self.received.range(..=wire).next_back()?;
        (wire < *end).then_some(*tuple)
    }

    fn packet(&mut self, ad: &[u8], payload: &[u8], sent: bool, wire: usize, now: Instant) {
        let Some(header) = parse_quic_header(ad) else {
            return;
        };
        if sent {
            let id = self.connection(&header, true, None);
            let unsent = match id {
                Some(id) => Unsent::Known(id),
                None => Unsent::Deferred {
                    ad: ad.to_vec(),
                    payload: payload.to_vec(),
                    at: now,
                },
            };
            if self.unsent.len() >= MAX_BUFFERS {
                if let Some(pending) = self.unsent.pop_front() {
                    self.flush(pending.unsent, None);
                }
            }
            self.unsent.push_back(Pending {
                wire,
                header_len: ad.len(),
                ciphertext: Vec::new(),
                unsent,
            });
            if id.is_some() {
                self.process(&header, id, ad, payload, true, now);
            }
            return;
        }

        let tuple = self.received_tuple(wire);
        let by_tuple = tuple.and_then(|t| self.tuple_connection(t));
        let id = self.connection(&header, false, by_tuple);
        if let (Some(id), Some(tuple)) = (id, tuple) {
            self.tuples.insert(tuple, id);
        }
        self.process(&header, id, ad, payload, false, now);
    }

    /// Processes a sent packet held back in `unsent`, of the connection
    /// with 4-tuple `tuple`.
    fn flush(&mut self, unsent: Unsent, tuple: Option<Tuple>) {
        match unsent {
            Unsent::Known(id) => {
                if let Some(tuple) = tuple {
                    self.tuples.insert(tuple, id);
                }
            }
            Unsent::Deferred { ad, payload, at } => {
                let Some(header) = parse_quic_header(&ad) else {
                    return;
                };
                let by_tuple = tuple.and_then(|t| self.tuple_connection(t));
                let id = by_tuple.and_then(|id| self.connection(&header, true, Some(id)));
                self.process(&header, id, &ad, &payload, true, at);
            }
        }
    }

    /// Records the ciphertext of the packet sealed at `wire`.
    fn sealed(&mut self, wire: usize, ciphertext: &[u8]) {
        let pending = self
            .unsent
            .iter_mut()
            .rev()
            .find(|p| p.wire == wire && p.ciphertext.is_empty());
        if let Some(pending) = pending {
            pending.ciphertext = ciphertext[..ciphertext.len().min(MAX_CIPHERTEXT)].to_vec();
        }
    }

    /// Handles the datagrams `bufs` that went through the UDP socket `fd` to
    /// or from `peer`.
    fn datagrams(&mut self, fd: i32, peer: Option<SocketAddr>, bufs: &[&[u8]], sent: bool) {
        let tuple = Tuple { fd, peer };
        if !sent {
            if self.received.len() >= MAX_BUFFERS {
                self.received.clear();
            }
            for buf in bufs {
                let (start, end) = (buf.as_ptr() as usize, buf.as_ptr() as usize + buf.len());
                let overlapping: Vec<usize> = self
                    .received
                    .range(..end)
                    .rev()
                    .take_while(|(_, (e, _))| *e > start)
                    .map(|(s, _)| *s)
                    .collect();
                for s in overlapping {
                    self.received.remove(&s);
                }
                self.received.insert(start, (end, tuple));
            }
            return;
        }

        let (sent, unsent): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.unsent)
            .into_iter()
            .partition(|p| bufs.iter().any(|buf| p.in_datagram(buf)));
        self.unsent = unsent;
        for pending in sent {
            self.flush(pending.unsent, Some(tuple));
        }
    }

    fn process(
        &mut self,
        header: &QuicHeader,
        id: Option<usize>,
        ad: &[u8],
        payload: &[u8],
        sent: bool,
        now: Instant,
    ) {
        // SAFETY: the estimator only keeps it as the time of its first sample.
        let timestamp = unsafe { Timestamp::from_duration(now - self.start) };
        let client = self.client;
        let mut unattributed = Connection::new(String::new());
        let conn = match id {
            Some(id) => &mut self.connections[id],
            None => &mut unattributed,
        };
        let dir = sent as usize;
        let space = space(header.packet_type);
        let pn = match space {
            Some(space) => conn.packet_number(dir, space, ad),
            None => header.packet_number,
        };

        let mut events = Vec::new();
        if header.packet_type == QuicPacketType::Short {
            let phase = ad[0] & 0x04 != 0;
            if conn.key_phase[dir].is_some_and(|old| old != phase) {
                conn.key_generation[dir] += 1;
                events.push(Extended::from(EventData::KeyUpdated(KeyUpdated {
                    key_type: if sent == client {
                        KeyType::Client1RttSecret
                    } else {
                        KeyType::Server1RttSecret
                    },
                    old: None,
                    new: String::new(),
                    generation: Some(conn.key_generation[dir]),
                    trigger: None,
                })));
            }
            conn.key_phase[dir] = Some(phase);
        }

        let mut frames = Vec::new();
        let mut eliciting = false;
        let mut new_cids = Vec::new();
        frame::walk(payload, |frame| {
            eliciting |= ack_eliciting(&frame);
            frames.push(qlog_frame(&frame, conn.ack_delay_exponent[dir]));
            let Frame::Quic(frame) = frame else {
                return;
            };
            match frame {
                FrameRef::Ack(ack) if !sent => {
                    if let Some(space) = space {
                        events.extend(
                            conn.on_ack(space, &ack, now, timestamp)
                                .into_iter()
                                .map(Extended::from),
                        );
                    }
                }
                FrameRef::Crypto(crypto) => {
                    let Some(stream) = space.and_then(|s| conn.crypto[dir].get_mut(s)) else {
                        return;
                    };
                    let data = crypto.data.into_less_safe_slice();
                    for Parameters {
                        set: mut params,
                        extra,
                    } in stream.push(crypto.offset.as_u64(), data)
                    {
                        if let Some(exponent) = params.ack_delay_exponent {
                            conn.ack_delay_exponent[dir] = exponent.min(20) as u8;
                        }
                        if let Some(delay) = params.max_ack_delay.filter(|_| !sent) {
                            let delay = Duration::from_millis(delay.into());
                            if let Ok(delay) = MaxAckDelay::try_from(delay) {
                                conn.rtt.on_max_ack_delay(delay);
                            }
                        }
                        params.owner = Some(if sent {
                            TransportOwner::Local
                        } else {
                            TransportOwner::Remote
                        });
                        events.push(Extended {
                            data: EventData::TransportParametersSet(params),
                            extra,
                        });
                    }
                }
                FrameRef::NewConnectionId(f) => new_cids.push(f.connection_id.to_vec()),
                FrameRef::HandshakeDone(_) => conn.handshake_confirmed = true,
                _ => {}
            }
        });

        let size = (ad.len() + payload.len() + TAG_LEN) as u64;
        if let (true, Some(space), Some(pn)) = (sent, space, pn) {
            conn.on_sent(
                space,
                pn,
                Sent {
                    at: now,
                    size,
                    ack_eliciting: eliciting,
                },
            );
            if eliciting {
                events.push(conn.metrics(false).into());
            }
        }
        let group_id = id.map(|_| conn.group_id.clone());
        if let Some(id) = id {
            let cids = if sent {
                &mut self.local_cids
            } else {
                &mut self.remote_cids
            };
            for cid in new_cids {
                cids.entry(cid).or_insert(id);
            }
        } else {
            // Without its connection, the derived events would be wrong.
            events.clear();
        }

        let qlog_header = PacketHeader::new(
            packet_type(header.packet_type),
            pn,
            Some(ad[0]),
            None,
            Some(payload.len().min(u16::MAX as usize) as u16),
            header.version,
            header.scid.as_deref(),
            Some(&header.dcid),
        );
        let raw = Some(RawInfo {
            length: Some(size),
            payload_length: Some(payload.len() as u64),
            data: None,
        });
        let packet = if sent {
            EventData::PacketSent(PacketSent {
                header: qlog_header,
                raw,
                frames: Some(frames.into()),
                ..Default::default()
            })
        } else {
            EventData::PacketReceived(PacketReceived {
                header: qlog_header,
                raw,
                frames: Some(frames),
                ..Default::default()
            })
        };

        for Extended { data, extra } in std::iter::once(packet.into()).chain(events) {
            let mut event = Event::with_time(0.0, data);
            event.group_id = group_id.clone();
            let _ = self
                .streamer
                .add_event_with_instant(WithExtraData { event, extra }, now);
        }
    }
}

/// Whether `NQ_QLOG` is set and the qlog file could be opened.
pub(crate) fn enabled() -> bool {
    QLOG.get_or_init(init_trace).is_some()
}

/// Flushes and closes the qlog trace, if one is being written.
pub(crate) fn finish() {
    if let Some(Some(trace)) = QLOG.get() {
        if let Ok(mut trace) = trace.lock() {
            for pending in std::mem::take(&mut trace.unsent) {
                trace.flush(pending.unsent, None);
            }
            let _ = trace.streamer.finish_log();
        }
    }
}

/// Records the packet with header `ad`, plaintext `payload` and ciphertext
/// at `wire`, `sent` or received, and what can be inferred from it.
pub(crate) fn emit_packet(ad: &[u8], payload: &[u8], sent: bool, wire: usize) {
    let Some(trace) = QLOG.get_or_init(init_trace) else {
        return;
    };
    let now = Instant::now();
    if let Ok(mut trace) = trace.lock() {
        trace.packet(ad, payload, sent, wire, now);
    }
}

/// Records the datagrams `bufs` that went through the UDP socket `fd` to or
/// from `peer`, which tells the 4-tuple of the packets in them.
pub(crate) fn on_datagrams(fd: i32, peer: Option<SocketAddr>, bufs: &[&[u8]], sent: bool) {
    if let Some(Some(trace)) = QLOG.get() {
        if let Ok(mut trace) = trace.lock() {
            trace.datagrams(fd, peer, bufs, sent);
        }
    }
}

/// Records `ciphertext`, the start of the packet just sealed at `wire`.
pub(crate) fn sealed(wire: usize, ciphertext: &[u8]) {
    if let Some(Some(trace)) = QLOG.get() {
        if let Ok(mut trace) = trace.lock() {
            trace.sealed(wire, ciphertext);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_extra_fields_to_data() {
        let data = EventData::TransportParametersSet(Default::default());
        let mut extra = Map::new();
        extra.insert("grease_quic_bit".into(), Value::Bool(true));
        let event = WithExtraData {
            event: Event::with_time(1.0, data),
            extra,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "transport:parameters_set");
        assert_eq!(json["data"]["grease_quic_bit"], true);
    }

    fn trace() -> Trace {
        let streamer = QlogStreamer::new(
            QLOG_VERSION.to_string(),
            None,
            None,
            None,
            Instant::now(),
            TraceSeq::new(
                VantagePoint {
                    name: None,
                    ty: VantagePointType::Client,
                    flow: None,
                },
                None,
                None,
                None,
                None,
            ),
            EventImportance::Extra,
            Box::new(std::io::sink()),
        );
        Trace {
            streamer,
            start: Instant::now(),
            client: true,
            connections: Vec::new(),
            local_cids: HashMap::new(),
            remote_cids: HashMap::new(),
            zero_len_local: Vec::new(),
            zero_len_remote: Vec::new(),
            tuples: HashMap::new(),
            received: BTreeMap::new(),
            unsent: VecDeque::new(),
        }
    }

    fn header(packet_type: QuicPacketType, dcid: &[u8], scid: Option<&[u8]>) -> QuicHeader {
        QuicHeader {
            packet_type,
            version: None,
            dcid: dcid.to_vec(),
            scid: scid.map(<[u8]>::to_vec),
            packet_number: None,
        }
    }

    #[test]
    fn tells_apart_connections_with_zero_length_ids() {
        use QuicPacketType::{Initial, Short};
        // A client without connection IDs of its own, as msquic's.
        let mut trace = trace();
        assert_eq!(
            trace.connection(&header(Initial, b"aa", Some(b"")), true, None),
            Some(0)
        );
        assert_eq!(
            trace.connection(&header(Initial, b"", Some(b"a1")), false, None),
            Some(0)
        );
        assert_eq!(
            trace.connection(&header(Short, b"", None), false, None),
            Some(0)
        );
        assert_eq!(
            trace.connection(&header(Initial, b"bb", Some(b"")), true, None),
            Some(1)
        );
        assert_eq!(
            trace.connection(&header(Short, b"a1", None), true, None),
            Some(0)
        );
        // Both connections now receive packets with an empty ID.
        assert_eq!(
            trace.connection(&header(Short, b"", None), false, None),
            None
        );
    }

    /// A v1 Initial header with `dcid`, an empty source connection ID and
    /// packet number 0.
    fn initial(dcid: &[u8]) -> Vec<u8> {
        [
            &[0xc0, 0, 0, 0, 1, dcid.len() as u8][..],
            dcid,
            &[0, 0, 0x02, 0],
        ]
        .concat()
    }

    #[test]
    fn attributes_zero_length_ids_by_tuple() {
        // A server with two clients without connection IDs, as msquic's.
        let mut trace = trace();
        let now = Instant::now();
        let received = [[0u8; 100]; 2];
        for (i, dcid) in [b"aa", b"bb"].into_iter().enumerate() {
            let peer = Some(SocketAddr::from(([10, 0, 0, i as u8], 4433)));
            let buf = &received[i][..];
            trace.datagrams(3, peer, &[buf], false);
            trace.packet(
                &initial(dcid),
                &[0x01],
                false,
                buf.as_ptr() as usize + 10,
                now,
            );
        }
        assert_eq!(trace.connections.len(), 2);

        // 1-RTT packets to the second client: only their datagrams tell,
        // whether sent from where they were sealed or as a copy.
        let peer = Some(SocketAddr::from(([10, 0, 0, 1], 4433)));
        let sealed = [0x40, 0x05, 1, 2, 3, 4, 5];
        let wire = sealed.as_ptr() as usize + 2;
        trace.packet(&[0x40, 0x05], &[0x01], true, wire, now);
        trace.packet(&[0x40, 0x06], &[0x01], true, 1, now);
        trace.sealed(1, &[6, 7, 8, 9]);
        assert!(trace.connections[1].unacked[2].is_empty());
        let copy = [0x4f, 0xff, 6, 7, 8, 9, 10];
        trace.datagrams(3, peer, &[&sealed[..], &copy[..]], true);
        assert!(trace.unsent.is_empty());
        assert!(trace.connections[0].unacked[2].is_empty());
        let pns: Vec<_> = trace.connections[1].unacked[2].keys().copied().collect();
        assert_eq!(pns, [5, 6]);
    }

    #[test]
    fn expands_packet_numbers() {
        let mut conn = Connection::new(String::new());
        // Short headers with a 1-byte packet number: 0xff, then 0x00.
        assert_eq!(conn.packet_number(1, 2, &[0x40, 0xff]), Some(0xff));
        assert_eq!(conn.packet_number(1, 2, &[0x40, 0x00]), Some(0x100));
        // Each direction and space counts on its own.
        assert_eq!(conn.packet_number(0, 2, &[0x40, 0x00]), Some(0));
    }

    #[test]
    fn samples_rtt_and_detects_losses() {
        let mut conn = Connection::new(String::new());
        let start = Instant::now();
        for pn in 0..=4 {
            let sent = Sent {
                at: start + Duration::from_millis(pn),
                size: 100,
                ack_eliciting: true,
            };
            conn.on_sent(2, pn, sent);
        }
        assert_eq!(conn.bytes_in_flight, 500);

        // ACK of packet 4 only, with an ACK delay of 1000 << 3 us.
        let now = start + Duration::from_millis(54);
        let mut events = Vec::new();
        frame::walk(&[0x02, 0x04, 0x43, 0xe8, 0x00, 0x00], |frame| {
            if let Frame::Quic(FrameRef::Ack(ack)) = frame {
                events = conn.on_ack(2, &ack, now, unsafe {
                    Timestamp::from_duration(now - start)
                });
            }
        });

        assert_eq!(conn.rtt.latest_rtt(), Duration::from_millis(50));
        let lost: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EventData::PacketLost(lost) => lost.header.as_ref()?.packet_number,
                _ => None,
            })
            .collect();
        // 0 and 1 by the packet threshold; 2 and 3 are too recent.
        assert_eq!(lost, [0, 1]);
        assert_eq!((conn.packets_in_flight, conn.bytes_in_flight), (2, 200));
    }
}
