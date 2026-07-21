//! Emits a qlog trace of observed QUIC packet headers.
//!
//! Enabled by setting `NESQUIC_QLOG` to an output path; if unset, no qlog
//! file is written and [`emit_packet`] is a no-op.

use std::fs::OpenOptions;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use qlog::events::quic::{PacketHeader, PacketReceived, PacketSent, PacketType};
use qlog::events::{Event, EventData, EventImportance};
use qlog::streamer::QlogStreamer;
use qlog::{Configuration, TraceSeq, VantagePoint, VantagePointType, QLOG_VERSION};

use super::{QuicHeader, QuicPacketType};

static QLOG: OnceLock<Option<Mutex<QlogStreamer>>> = OnceLock::new();

fn init_streamer() -> Option<Mutex<QlogStreamer>> {
    let path = std::env::var("NQ_QLOG").ok()?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;

    let trace = TraceSeq::new(
        VantagePoint {
            name: None,
            ty: VantagePointType::Unknown,
            flow: None,
        },
        Some("nesquic".to_string()),
        Some("QUIC headers observed via crypto hooks".to_string()),
        Some(Configuration {
            time_offset: Some(0.0),
            original_uris: None,
        }),
        None,
    );

    let mut streamer = QlogStreamer::new(
        QLOG_VERSION.to_string(),
        Some("nesquic".to_string()),
        None,
        None,
        Instant::now(),
        trace,
        EventImportance::Core,
        Box::new(file),
    );

    streamer.start_log().ok()?;
    Some(Mutex::new(streamer))
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

/// Records `header` as a `packet_sent` (if `sent`) or `packet_received` qlog
/// event, grouped by its destination connection ID. `len` is the packet's
/// on-wire length (the AEAD ciphertext length, since that's what the crypto
/// hooks observe).
pub(crate) fn emit_packet(header: &QuicHeader, len: usize, sent: bool) {
    let Some(streamer) = QLOG.get_or_init(init_streamer) else {
        return;
    };

    let qlog_header = PacketHeader::new(
        packet_type(header.packet_type),
        header.packet_number,
        None,
        None,
        Some(len.min(u16::MAX as usize) as u16),
        header.version,
        header.scid.as_deref(),
        Some(&header.dcid),
    );

    let event_data = if sent {
        EventData::PacketSent(PacketSent {
            header: qlog_header,
            ..Default::default()
        })
    } else {
        EventData::PacketReceived(PacketReceived {
            header: qlog_header,
            ..Default::default()
        })
    };

    let mut event = Event::with_time(0.0, event_data);
    event.group_id = Some(super::hex(&header.dcid));

    let mut streamer = streamer.lock().unwrap();
    let _ = streamer.add_event_with_instant(event, Instant::now());
}
