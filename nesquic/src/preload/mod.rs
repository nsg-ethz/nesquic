use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

use bytes::{Buf, BytesMut};
use quinn_proto::{ConnectionId, ConnectionIdParser, LongType, PacketDecodeError, ProtectedHeader};

mod qlog;
mod quiche;
mod quinn;

static REPORTER: Once = Once::new();

/// Register the exit reporter the first time any crypto function is hooked, so
/// processes that never touch the monitored library produce no output.
pub(crate) fn arm_reporter() {
    REPORTER.call_once(|| unsafe {
        libc::atexit(report);
    });
}

extern "C" fn report() {
    // TODO: parse qlog and upload it to influxDB
    println!("nesquic upload");
}

/// The type of a QUIC packet, as encoded in the two type-specific bits of a
/// long header's first byte (RFC 9000 section 17.2), or `Short` for 1-RTT
/// packets using the short header form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuicPacketType {
    Initial,
    ZeroRtt,
    Handshake,
    Retry,
    VersionNegotiation,
    Short,
}

/// Fields of a QUIC packet header, as seen in the AEAD associated data before
/// header protection is applied (i.e. with the real packet number in the
/// clear).
///
/// `packet_number` is the *truncated* packet number as it appears on the
/// wire, not the full reconstructed packet number (which requires tracking
/// the largest packet number seen so far).
#[derive(Debug)]
pub(crate) struct QuicHeader {
    pub packet_type: QuicPacketType,
    pub version: Option<u32>,
    pub dcid: Vec<u8>,
    pub scid: Option<Vec<u8>>,
    pub packet_number: Option<u64>,
}

/// The maximum length of a QUIC connection ID (RFC 9000 section 17.2):
/// `len` is encoded in the first byte of long-header DCID/SCID fields as a
/// single unsigned byte's worth of bits, but the protocol additionally caps
/// it at 20 bytes. `quinn_proto::ConnectionId` bakes in the same limit
/// (as a private `MAX_CID_SIZE` constant) and silently corrupts its
/// fixed-size backing array if constructed with a longer one in a release
/// build (no bounds check outside of `debug_assert!`), so this *must* be
/// enforced before calling `ConnectionId::from_buf`.
const MAX_CID_LEN: usize = 20;

/// A [`ConnectionIdParser`] for short headers, which don't encode the DCID
/// length on the wire. Since `buf` (see [`parse_quic_header`]) is known to
/// be exactly the unprotected header with no trailing payload, the DCID is
/// everything left over once the trailing `pn_len`-byte packet number is
/// accounted for -- assuming that's plausible; if not (e.g. `buf` wasn't
/// really an isolated header after all), reject rather than feeding a
/// bogus length to `ConnectionId::from_buf`.
struct RemainderConnectionIdParser {
    pn_len: usize,
}

impl ConnectionIdParser for RemainderConnectionIdParser {
    fn parse(&self, buf: &mut dyn Buf) -> Result<ConnectionId, PacketDecodeError> {
        let dcid_len = buf
            .remaining()
            .checked_sub(self.pn_len)
            .ok_or(PacketDecodeError::InvalidHeader("packet too small"))?;
        (dcid_len <= MAX_CID_LEN)
            .then(|| ConnectionId::from_buf(buf, dcid_len))
            .ok_or(PacketDecodeError::InvalidHeader("dcid too long"))
    }
}

/// Parses a QUIC packet header from `buf`, which is expected to be exactly
/// the unprotected header (e.g. the AEAD associated data passed to a seal or
/// open call), with no trailing payload.
pub(crate) fn parse_quic_header(buf: &[u8]) -> Option<QuicHeader> {
    let first = *buf.first()?;
    let pn_len = (first & 0x03) as usize + 1;

    // The version, present only for long headers, doubles as the sole
    // "supported" version passed to `ProtectedHeader::decode` below: this
    // parser just observes packets a real QUIC stack already selected, so
    // there's no version negotiation to enforce here.
    let version = if first & 0x80 != 0 {
        Some(u32::from_be_bytes(buf.get(1..5)?.try_into().ok()?))
    } else {
        None
    };
    let supported_versions = [version.unwrap_or(0)];

    let mut cursor = io::Cursor::new(BytesMut::from(buf));
    let cid_parser = RemainderConnectionIdParser { pn_len };
    // `grease_quic_bit: true` skips the fixed-bit check, matching the
    // previous hand-rolled parser, which didn't enforce it either.
    let plain_header =
        ProtectedHeader::decode(&mut cursor, &cid_parser, &supported_versions, true).ok()?;

    let packet_type = match &plain_header {
        ProtectedHeader::Initial(_) => QuicPacketType::Initial,
        ProtectedHeader::Long {
            ty: LongType::ZeroRtt,
            ..
        } => QuicPacketType::ZeroRtt,
        ProtectedHeader::Long {
            ty: LongType::Handshake,
            ..
        } => QuicPacketType::Handshake,
        ProtectedHeader::Retry { .. } => QuicPacketType::Retry,
        ProtectedHeader::VersionNegotiate { .. } => QuicPacketType::VersionNegotiation,
        ProtectedHeader::Short { .. } => QuicPacketType::Short,
    };

    let dcid = plain_header.dst_cid().to_vec();
    let scid = match &plain_header {
        ProtectedHeader::Initial(h) => Some(h.src_cid.to_vec()),
        ProtectedHeader::Long { src_cid, .. } => Some(src_cid.to_vec()),
        ProtectedHeader::Retry { src_cid, .. } => Some(src_cid.to_vec()),
        ProtectedHeader::VersionNegotiate { src_cid, .. } => Some(src_cid.to_vec()),
        ProtectedHeader::Short { .. } => None,
    };

    // Retry and version-negotiation packets carry no packet number. For the
    // others, `decode` stops right before it (the packet number length is
    // normally masked by header protection); read it directly off the
    // trailing bytes, which are known to be unprotected here.
    let packet_number = match packet_type {
        QuicPacketType::Retry | QuicPacketType::VersionNegotiation => None,
        _ => {
            let pos = cursor.position() as usize;
            Some(read_packet_number(cursor.get_ref().get(pos..pos + pn_len)?))
        }
    };

    Some(QuicHeader {
        packet_type,
        version,
        dcid,
        scid,
        packet_number,
    })
}

fn read_packet_number(buf: &[u8]) -> u64 {
    buf.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
