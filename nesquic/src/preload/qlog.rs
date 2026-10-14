//! Emits a qlog trace of observed QUIC packets: their headers and STREAM
//! frames, so that viewers can show how streams are multiplexed.
//!
//! Enabled by setting `NQ_QLOG` to an output path; if unset, no qlog file
//! is written. This is meant for debugging only: every packet goes through a
//! global lock and a serializer, which slows down the monitored library.

use std::fs::OpenOptions;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use qlog::events::quic::{PacketHeader, PacketReceived, PacketSent, PacketType, QuicFrame};
use qlog::events::{Event, EventData, EventImportance};
use qlog::streamer::QlogStreamer;
use qlog::{Configuration, TraceSeq, VantagePoint, VantagePointType, QLOG_VERSION};

use super::context::Context;
use super::frame::StreamFrame;
use super::{QuicHeader, QuicPacketType};

static QLOG: OnceLock<Option<Mutex<QlogStreamer>>> = OnceLock::new();

fn init_streamer() -> Option<Mutex<QlogStreamer>> {
    let path = std::env::var("NQ_QLOG").ok()?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;

    // Viewers need the vantage point to tell which packets went which way.
    let ty = match Context::current().mode.as_deref() {
        Some("client") => VantagePointType::Client,
        Some("server") => VantagePointType::Server,
        _ => VantagePointType::Unknown,
    };
    let trace = TraceSeq::new(
        VantagePoint {
            name: None,
            ty,
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

fn stream_frame(frame: &StreamFrame) -> QuicFrame {
    QuicFrame::Stream {
        stream_id: frame.id,
        offset: frame.offset,
        length: frame.len,
        fin: Some(frame.fin),
        raw: None,
    }
}

/// Whether `NQ_QLOG` is set and the qlog file could be opened.
pub(crate) fn enabled() -> bool {
    QLOG.get_or_init(init_streamer).is_some()
}

/// Flushes and closes the qlog trace, if one is being written.
pub(crate) fn finish() {
    if let Some(Some(streamer)) = QLOG.get() {
        if let Ok(mut streamer) = streamer.lock() {
            let _ = streamer.finish_log();
        }
    }
}

/// Records `header` as a `packet_sent` (if `sent`) or `packet_received` qlog
/// event with the packet's `streams` frames, grouped by its destination
/// connection ID. `len` is the packet's payload length, as observed by the
/// crypto hooks.
pub(crate) fn emit_packet(header: &QuicHeader, len: usize, streams: &[StreamFrame], sent: bool) {
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

    let frames = streams.iter().map(stream_frame);
    let event_data = if sent {
        EventData::PacketSent(PacketSent {
            header: qlog_header,
            frames: Some(frames.collect()),
            ..Default::default()
        })
    } else {
        EventData::PacketReceived(PacketReceived {
            header: qlog_header,
            frames: Some(frames.collect()),
            ..Default::default()
        })
    };

    let mut event = Event::with_time(0.0, event_data);
    event.group_id = Some(super::hex(&header.dcid));

    let mut streamer = streamer.lock().unwrap();
    let _ = streamer.add_event_with_instant(event, Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_stream_frames() {
        let frame = StreamFrame {
            id: 4,
            offset: 9,
            len: 3,
            fin: true,
        };
        let json = serde_json::to_string(&stream_frame(&frame)).unwrap();
        assert!(json.contains(r#""frame_type":"stream""#), "{json}");
        assert!(json.contains(r#""stream_id":4"#), "{json}");
        assert!(json.contains(r#""offset":9"#), "{json}");
        assert!(json.contains(r#""length":3"#), "{json}");
        assert!(json.contains(r#""fin":true"#), "{json}");
    }
}
