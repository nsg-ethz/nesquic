use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

mod quiche;
mod quinn;

pub(crate) static SEAL_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static SEAL_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static OPEN_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static OPEN_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static AES_HP_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static CHACHA_HP_CALLS: AtomicU64 = AtomicU64::new(0);

static REPORTER: Once = Once::new();

/// Register the exit reporter the first time any crypto function is hooked, so
/// processes that never touch the monitored library produce no output.
pub(crate) fn arm_reporter() {
    REPORTER.call_once(|| unsafe {
        libc::atexit(report);
    });
}

extern "C" fn report() {
    let summary = format!(
        "nesquic pid={} seal_calls={} seal_bytes={} open_calls={} open_bytes={} aes_hp_calls={} chacha_hp_calls={}\n",
        std::process::id(),
        SEAL_CALLS.load(Ordering::Relaxed),
        SEAL_BYTES.load(Ordering::Relaxed),
        OPEN_CALLS.load(Ordering::Relaxed),
        OPEN_BYTES.load(Ordering::Relaxed),
        AES_HP_CALLS.load(Ordering::Relaxed),
        CHACHA_HP_CALLS.load(Ordering::Relaxed),
    );

    if let Ok(path) = std::env::var("NESQUIC_OUT") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(summary.as_bytes());
            return;
        }
    }
    let _ = std::io::stderr().write_all(summary.as_bytes());
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

/// Reads a QUIC variable-length integer (RFC 9000 section 16) from the start
/// of `buf`, returning the decoded value and the number of bytes consumed.
fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let first = *buf.first()?;
    let len = 1usize << (first >> 6);
    let bytes = buf.get(..len)?;

    let mut val = (first & 0x3f) as u64;
    for &b in &bytes[1..] {
        val = (val << 8) | b as u64;
    }
    Some((val, len))
}

/// Parses a QUIC packet header from `buf`, which is expected to be exactly
/// the unprotected header (e.g. the AEAD associated data passed to a seal or
/// open call), with no trailing payload.
pub(crate) fn parse_quic_header(buf: &[u8]) -> Option<QuicHeader> {
    let first = *buf.first()?;
    let pn_len = (first & 0x03) as usize + 1;

    if first & 0x80 == 0 {
        // Short header: form is `1S00KPPP`, immediately followed by the DCID
        // and the packet number. The DCID length isn't encoded in the
        // packet, but since `buf` is exactly the header, it's everything
        // between the first byte and the trailing packet number.
        let dcid_end = buf.len().checked_sub(pn_len)?;
        let dcid = buf.get(1..dcid_end)?.to_vec();
        let packet_number = read_packet_number(&buf[dcid_end..]);

        return Some(QuicHeader {
            packet_type: QuicPacketType::Short,
            version: None,
            dcid,
            scid: None,
            packet_number: Some(packet_number),
        });
    }

    // Long header: form is `1TTPPPPP` followed by version, DCID, SCID and
    // type-specific fields.
    let version = u32::from_be_bytes(buf.get(1..5)?.try_into().ok()?);
    let packet_type = if version == 0 {
        QuicPacketType::VersionNegotiation
    } else {
        match (first & 0x30) >> 4 {
            0x00 => QuicPacketType::Initial,
            0x01 => QuicPacketType::ZeroRtt,
            0x02 => QuicPacketType::Handshake,
            _ => QuicPacketType::Retry,
        }
    };

    let mut pos = 5;
    let dcid_len = *buf.get(pos)? as usize;
    pos += 1;
    let dcid = buf.get(pos..pos + dcid_len)?.to_vec();
    pos += dcid_len;

    let scid_len = *buf.get(pos)? as usize;
    pos += 1;
    let scid = buf.get(pos..pos + scid_len)?.to_vec();
    pos += scid_len;

    // Retry and version-negotiation packets carry no packet number.
    if matches!(
        packet_type,
        QuicPacketType::Retry | QuicPacketType::VersionNegotiation
    ) {
        return Some(QuicHeader {
            packet_type,
            version: Some(version),
            dcid,
            scid: Some(scid),
            packet_number: None,
        });
    }

    // Initial packets carry a token, length-prefixed by a varint.
    if packet_type == QuicPacketType::Initial {
        let (token_len, varint_len) = read_varint(&buf[pos..])?;
        pos += varint_len + token_len as usize;
    }

    // Length: varint giving the size of the remaining packet number + payload.
    let (_length, varint_len) = read_varint(&buf[pos..])?;
    pos += varint_len;

    let packet_number = read_packet_number(buf.get(pos..pos + pn_len)?);

    Some(QuicHeader {
        packet_type,
        version: Some(version),
        dcid,
        scid: Some(scid),
        packet_number: Some(packet_number),
    })
}

fn read_packet_number(buf: &[u8]) -> u64 {
    buf.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Prints the "5-tuple" identifying a QUIC packet header: packet type,
/// version, destination/source connection IDs, and packet number. Fields
/// that don't apply to the packet's header form (e.g. `version`/`scid` for
/// short headers) are printed as `-`.
pub(crate) fn print_quic_header_5_tuple(header: &QuicHeader) {
    eprintln!(
        "quic_header type={:?} version={} dcid={} scid={} packet_number={}",
        header.packet_type,
        header
            .version
            .map(|v| format!("0x{v:08x}"))
            .unwrap_or_else(|| "-".to_string()),
        hex(&header.dcid),
        header
            .scid
            .as_deref()
            .map(hex)
            .unwrap_or_else(|| "-".to_string()),
        header
            .packet_number
            .map(|pn| pn.to_string())
            .unwrap_or_else(|| "-".to_string()),
    );
}
