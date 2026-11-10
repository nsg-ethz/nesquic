//! Walks the frames of a decrypted packet payload with the frame codec of
//! `s2n-quic-core` (RFC 9000 section 19 and DATAGRAM), plus the ACK frequency
//! extension frames, which it does not know.

use std::cell::RefCell;

use s2n_codec::{DecoderBuffer, DecoderBufferMut};
use s2n_quic_core::frame::FrameRef;
use s2n_quic_core::varint::VarInt;

/// IMMEDIATE_ACK (draft-ietf-quic-ack-frequency).
pub(crate) const IMMEDIATE_ACK: u64 = 0x1f;
/// ACK_FREQUENCY (draft-ietf-quic-ack-frequency).
pub(crate) const ACK_FREQUENCY: u64 = 0xaf;

/// A frame of a decrypted packet payload.
pub(crate) enum Frame<'a> {
    Quic(FrameRef<'a>),
    /// An extension frame that `s2n-quic-core` does not know, by type.
    Extension(u64),
}

/// What a decrypted packet payload carries.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Number of ACK frames.
    pub acks: u64,
    /// Stream data bytes, summed over all STREAM frames.
    pub stream_bytes: u64,
    /// Whether any STREAM frame has the FIN bit set.
    pub stream_fin: bool,
}

/// A STREAM frame, without its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamFrame {
    pub id: u64,
    pub offset: u64,
    pub len: u64,
    pub fin: bool,
}

thread_local! {
    /// The codec decodes from a mutable buffer, but the hooks only get the
    /// plaintext as `&[u8]`: payloads are copied here first.
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The length of the ACK frequency extension frame at the start of `buf`,
/// and its type.
fn extension_frame(buf: DecoderBuffer) -> Option<(usize, u64)> {
    let len = buf.len();
    let (ty, buf) = buf.decode::<VarInt>().ok()?;
    let fields = match ty.as_u64() {
        IMMEDIATE_ACK => 0,
        // Sequence number, ack-eliciting threshold, requested max ACK delay
        // and reordering threshold.
        ACK_FREQUENCY => 4,
        _ => return None,
    };
    let mut buf = buf;
    for _ in 0..fields {
        buf = buf.decode::<VarInt>().ok()?.1;
    }
    Some((len - buf.len(), ty.as_u64()))
}

/// Calls `on_frame` for every frame in the decrypted packet `payload`, in
/// order. Walking stops at the first unknown or malformed frame.
pub(crate) fn walk(payload: &[u8], mut on_frame: impl FnMut(Frame<'_>)) {
    SCRATCH.with_borrow_mut(|scratch| {
        scratch.clear();
        scratch.extend_from_slice(payload);
        let mut buf = DecoderBufferMut::new(scratch);
        while !buf.is_empty() {
            if let Some((len, ty)) = extension_frame(buf.peek()) {
                let Ok(rest) = buf.skip(len) else {
                    return;
                };
                on_frame(Frame::Extension(ty));
                buf = rest;
                continue;
            }
            let Ok((frame, rest)) = buf.decode::<FrameRef>() else {
                return;
            };
            on_frame(Frame::Quic(frame));
            buf = rest;
        }
    });
}

/// Summarizes the frames in a decrypted packet `payload`, calling
/// `on_stream` for every STREAM frame. Only the frames before the first
/// unknown or malformed one are counted.
pub(crate) fn summarize(payload: &[u8], mut on_stream: impl FnMut(StreamFrame)) -> Summary {
    let mut summary = Summary::default();
    walk(payload, |frame| match frame {
        Frame::Quic(FrameRef::Ack(_)) => summary.acks += 1,
        Frame::Quic(FrameRef::Stream(stream)) => {
            let frame = StreamFrame {
                id: stream.stream_id.as_u64(),
                offset: stream.offset.as_u64(),
                len: stream.data.len() as u64,
                fin: stream.is_fin,
            };
            summary.stream_bytes += frame.len;
            summary.stream_fin |= frame.fin;
            on_stream(frame);
        }
        _ => {}
    });
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summarize(payload: &[u8]) -> Summary {
        super::summarize(payload, |_| {})
    }

    #[test]
    fn reports_stream_frames() {
        let payload = [
            0x0e, 0x04, 0x09, 0x03, 0xaa, 0xbb, 0xcc, // STREAM 4, OFF 9, LEN 3
            0x0b, 0x08, 0x02, 0xdd, 0xee, // STREAM 8, LEN 2, FIN
        ];
        let mut frames = Vec::new();
        super::summarize(&payload, |frame| frames.push(frame));
        assert_eq!(
            frames,
            [
                StreamFrame {
                    id: 4,
                    offset: 9,
                    len: 3,
                    fin: false
                },
                StreamFrame {
                    id: 8,
                    offset: 0,
                    len: 2,
                    fin: true
                },
            ]
        );
    }

    fn count_acks(payload: &[u8]) -> u64 {
        summarize(payload).acks
    }

    #[test]
    fn counts_acks() {
        let payload = [
            0x02, 0x05, 0x00, 0x01, 0x03, 0x00, 0x00, // ACK 5-2, 0
            0x01, // PING
            0x03, 0x09, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, // ACK_ECN
            0x0e, 0x00, 0x04, 0x03, 0xaa, 0xbb, 0xcc, // STREAM with LEN
            0x00, 0x00, // PADDING
        ];
        assert_eq!(count_acks(&payload), 2);
    }

    #[test]
    fn sums_stream_data_and_fin() {
        let payload = [
            0x0e, 0x00, 0x04, 0x03, 0xaa, 0xbb, 0xcc, // STREAM, OFF, LEN 3
            0x0b, 0x00, 0x02, 0xdd, 0xee, // STREAM, LEN 2, FIN
        ];
        assert_eq!(
            summarize(&payload),
            Summary {
                acks: 0,
                stream_bytes: 5,
                stream_fin: true
            }
        );
        // No LEN: the data is the rest of the packet.
        assert_eq!(summarize(&[0x08, 0x00, 1, 2, 3, 4]).stream_bytes, 4);
        // An empty STREAM frame with FIN carries no data.
        let fin_only = summarize(&[0x0f, 0x00, 0x08, 0x00]);
        assert_eq!((fin_only.stream_bytes, fin_only.stream_fin), (0, true));
    }

    #[test]
    fn stream_without_length_ends_packet() {
        // STREAM (no LEN) swallows the rest, even bytes that look like ACKs.
        assert_eq!(count_acks(&[0x08, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00]), 0);
    }

    #[test]
    fn skips_extension_frames() {
        let payload = [
            0x40, 0xaf, 0x01, 0x02, 0x03, 0x04, // ACK_FREQUENCY
            0x1f, // IMMEDIATE_ACK
            0x18, 0x01, 0x00, 0x02, 0xab, 0xcd, // NEW_CONNECTION_ID ...
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // ... reset token
            0x02, 0x00, 0x00, 0x00, 0x00, // ACK
        ];
        assert_eq!(count_acks(&payload), 1);
    }

    #[test]
    fn stops_at_unknown_or_truncated_frames() {
        assert_eq!(count_acks(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x21, 0x02]), 1);
        assert_eq!(count_acks(&[0x02, 0x00, 0x00]), 0);
    }
}
