//! A minimal QUIC frame walker (RFC 9000 section 19, plus the ACK
//! frequency, reliable reset and datagram extensions), just enough to find
//! the ACK and STREAM frames in a decrypted packet payload.
//!
//! QUIC frames carry no generic length, so a frame of unknown type cannot be
//! skipped: the walk ends there.

/// Reads a QUIC variable-length integer (RFC 9000 section 16).
fn varint(buf: &mut &[u8]) -> Option<u64> {
    let first = *buf.first()?;
    let len = 1usize << (first >> 6);
    let bytes = buf.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold((first & 0x3f) as u64, |acc, &b| (acc << 8) | b as u64);
    *buf = &buf[len..];
    Some(value)
}

fn skip(buf: &mut &[u8], n: u64) -> Option<()> {
    let n = usize::try_from(n).ok()?;
    *buf = buf.get(n..)?;
    Some(())
}

fn skip_varints(buf: &mut &[u8], n: usize) -> Option<()> {
    for _ in 0..n {
        varint(buf)?;
    }
    Some(())
}

/// Skips a varint length followed by that many bytes.
fn skip_prefixed(buf: &mut &[u8]) -> Option<()> {
    let len = varint(buf)?;
    skip(buf, len)
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

/// An ACK or ACK_ECN frame. The additional ACK ranges are kept encoded and
/// only decoded by [`AckFrame::ranges`], so that counting ACKs never
/// allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AckFrame<'a> {
    pub largest: u64,
    /// The encoded ACK delay; scaling it needs the peer's
    /// `ack_delay_exponent` transport parameter, which is not known here.
    pub delay: u64,
    first_range: u64,
    range_count: u64,
    /// The `range_count` (gap, ACK range length) varint pairs.
    ranges: &'a [u8],
    /// ECT0, ECT1 and ECN-CE counts of an ACK_ECN frame.
    pub ecn: Option<[u64; 3]>,
}

impl AckFrame<'_> {
    /// The acknowledged packet numbers as inclusive `(smallest, largest)`
    /// ranges, in descending order. Stops at the first malformed range.
    pub fn ranges(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        let mut buf = self.ranges;
        let mut remaining = self.range_count;
        let mut next = self
            .largest
            .checked_sub(self.first_range)
            .map(|smallest| (smallest, self.largest));
        std::iter::from_fn(move || {
            let current = next.take()?;
            if remaining > 0 {
                remaining -= 1;
                let gap = varint(&mut buf);
                let len = varint(&mut buf);
                next = gap.zip(len).and_then(|(gap, len)| {
                    let largest = current.0.checked_sub(gap)?.checked_sub(2)?;
                    Some((largest.checked_sub(len)?, largest))
                });
            }
            Some(current)
        })
    }
}

/// A STREAM frame, without its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamFrame {
    pub id: u64,
    pub offset: u64,
    pub len: u64,
    pub fin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frame<'a> {
    Ack(AckFrame<'a>),
    Stream(StreamFrame),
    /// A known frame that is neither ACK nor STREAM.
    Other,
    /// A frame of unknown type, with which the walk ends.
    Unknown(u64),
}

/// Summarizes the frames in a decrypted packet `payload`, calling
/// `on_frame` for every frame.
///
/// Frames are walked in order; the walk ends at the first unknown frame
/// (still passed to `on_frame`) or malformed frame (not passed), in which
/// case only the frames before it are counted.
pub(crate) fn summarize<'a>(
    mut payload: &'a [u8],
    mut on_frame: impl FnMut(&Frame<'a>),
) -> Summary {
    let mut summary = Summary::default();
    while !payload.is_empty() {
        let Some(frame) = next_frame(&mut payload) else {
            break;
        };
        on_frame(&frame);
        match frame {
            Frame::Ack(_) => summary.acks += 1,
            Frame::Stream(stream) => {
                summary.stream_bytes += stream.len;
                summary.stream_fin |= stream.fin;
            }
            Frame::Other => {}
            Frame::Unknown(_) => break,
        }
    }
    summary
}

/// Advances `buf` past one frame and returns what it was.
fn next_frame<'a>(buf: &mut &'a [u8]) -> Option<Frame<'a>> {
    let ty = varint(buf)?;
    match ty {
        // PADDING, PING, HANDSHAKE_DONE, IMMEDIATE_ACK
        0x00 | 0x01 | 0x1e | 0x1f => {}
        // ACK, ACK_ECN
        0x02 | 0x03 => {
            let largest = varint(buf)?;
            let delay = varint(buf)?;
            let range_count = varint(buf)?;
            let first_range = varint(buf)?;
            let start = *buf;
            for _ in 0..range_count {
                skip_varints(buf, 2)?; // gap, ack range length
            }
            let ranges = &start[..start.len() - buf.len()];
            let ecn = if ty == 0x03 {
                Some([varint(buf)?, varint(buf)?, varint(buf)?])
            } else {
                None
            };
            return Some(Frame::Ack(AckFrame {
                largest,
                delay,
                first_range,
                range_count,
                ranges,
                ecn,
            }));
        }
        // RESET_STREAM
        0x04 => skip_varints(buf, 3)?,
        // STOP_SENDING
        0x05 => skip_varints(buf, 2)?,
        // CRYPTO
        0x06 => {
            varint(buf)?;
            skip_prefixed(buf)?;
        }
        // NEW_TOKEN
        0x07 => skip_prefixed(buf)?,
        // STREAM
        0x08..=0x0f => {
            let id = varint(buf)?;
            let offset = if ty & 0x04 != 0 { varint(buf)? } else { 0 };
            let len = if ty & 0x02 != 0 {
                let len = varint(buf)?;
                skip(buf, len)?;
                len
            } else {
                // Without LEN, the data runs to the end of the packet.
                let len = buf.len() as u64;
                *buf = &[];
                len
            };
            return Some(Frame::Stream(StreamFrame {
                id,
                offset,
                len,
                fin: ty & 0x01 != 0,
            }));
        }
        // MAX_DATA, MAX_STREAMS, DATA_BLOCKED, STREAMS_BLOCKED,
        // RETIRE_CONNECTION_ID
        0x10 | 0x12 | 0x13 | 0x14 | 0x16 | 0x17 | 0x19 => skip_varints(buf, 1)?,
        // MAX_STREAM_DATA, STREAM_DATA_BLOCKED
        0x11 | 0x15 => skip_varints(buf, 2)?,
        // NEW_CONNECTION_ID
        0x18 => {
            skip_varints(buf, 2)?;
            let cid_len = *buf.first()?;
            skip(buf, 1 + cid_len as u64 + 16)?;
        }
        // PATH_CHALLENGE, PATH_RESPONSE
        0x1a | 0x1b => skip(buf, 8)?,
        // CONNECTION_CLOSE (transport, application)
        0x1c | 0x1d => {
            skip_varints(buf, if ty == 0x1c { 2 } else { 1 })?;
            skip_prefixed(buf)?;
        }
        // DATAGRAM
        0x30 => *buf = &[],
        0x31 => skip_prefixed(buf)?,
        // RESET_STREAM_AT
        0x24 => skip_varints(buf, 4)?,
        // ACK_FREQUENCY
        0xaf => skip_varints(buf, 4)?,
        _ => return Some(Frame::Unknown(ty)),
    }
    Some(Frame::Other)
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
        super::summarize(&payload, |frame| {
            if let Frame::Stream(stream) = frame {
                frames.push(*stream);
            }
        });
        assert_eq!(
            frames,
            [
                StreamFrame { id: 4, offset: 9, len: 3, fin: false },
                StreamFrame { id: 8, offset: 0, len: 2, fin: true },
            ]
        );
    }

    #[test]
    fn reads_varints() {
        // Examples from RFC 9000 appendix A.1.
        for (bytes, value) in [
            (&[0x25][..], 37),
            (&[0x7b, 0xbd], 15293),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494878333),
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
                151288809941952652,
            ),
        ] {
            let mut buf = bytes;
            assert_eq!(varint(&mut buf), Some(value));
            assert!(buf.is_empty());
        }
        assert_eq!(varint(&mut &[0x40][..]), None);
    }

    fn count_acks(payload: &[u8]) -> u64 {
        summarize(payload).acks
    }

    #[test]
    fn counts_acks() {
        let payload = [
            0x02, 0x05, 0x00, 0x01, 0x03, 0x00, 0x01, // ACK, one extra range
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

        let mut frames = Vec::new();
        super::summarize(&[0x01, 0x21, 0x02, 0x00, 0x00, 0x00, 0x00], |frame| {
            frames.push(*frame)
        });
        assert_eq!(frames, [Frame::Other, Frame::Unknown(0x21)]);
    }

    fn acks(payload: &[u8]) -> Vec<(Vec<(u64, u64)>, Option<[u64; 3]>)> {
        let mut acks = Vec::new();
        super::summarize(payload, |frame| {
            if let Frame::Ack(ack) = frame {
                acks.push((ack.ranges().collect(), ack.ecn));
            }
        });
        acks
    }

    #[test]
    fn decodes_ack_ranges() {
        // Largest 20, first range 2 (18-20), gap 1 + range 3 (12-15), gap 0 +
        // range 0 (10).
        let payload = [0x02, 20, 0x05, 0x02, 0x02, 0x01, 0x03, 0x00, 0x00];
        assert_eq!(acks(&payload), [(vec![(18, 20), (12, 15), (10, 10)], None)]);
        // ACK_ECN with a single range.
        let payload = [0x03, 0x07, 0x00, 0x00, 0x07, 0x01, 0x02, 0x03];
        assert_eq!(acks(&payload), [(vec![(0, 7)], Some([1, 2, 3]))]);
        // Ranges below packet number 0 are malformed and dropped.
        let payload = [0x02, 0x03, 0x00, 0x01, 0x01, 0x05, 0x00];
        assert_eq!(acks(&payload), [(vec![(2, 3)], None)]);
    }

    #[test]
    fn skips_reset_stream_at() {
        let payload = [0x24, 0x04, 0x00, 0x08, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(count_acks(&payload), 1);
    }
}
