//! A minimal QUIC frame walker (RFC 9000 section 19, plus the ACK
//! frequency extension), just enough to find the ACK and STREAM frames in a
//! decrypted packet payload.

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

enum Frame {
    Ack,
    Stream { len: u64, fin: bool },
    Other,
}

/// Summarizes the frames in a decrypted packet `payload`.
///
/// Frames are walked in order; parsing stops at the first unknown or
/// malformed frame, in which case only the frames before it are counted.
pub(crate) fn summarize(mut payload: &[u8]) -> Summary {
    let mut summary = Summary::default();
    while !payload.is_empty() {
        match next_frame(&mut payload) {
            Some(Frame::Ack) => summary.acks += 1,
            Some(Frame::Stream { len, fin }) => {
                summary.stream_bytes += len;
                summary.stream_fin |= fin;
            }
            Some(Frame::Other) => {}
            None => break,
        }
    }
    summary
}

/// Advances `buf` past one frame and returns what it was.
fn next_frame(buf: &mut &[u8]) -> Option<Frame> {
    let ty = varint(buf)?;
    match ty {
        // PADDING, PING, HANDSHAKE_DONE, IMMEDIATE_ACK
        0x00 | 0x01 | 0x1e | 0x1f => {}
        // ACK, ACK_ECN
        0x02 | 0x03 => {
            varint(buf)?; // largest acknowledged
            varint(buf)?; // ack delay
            let ranges = varint(buf)?;
            varint(buf)?; // first ack range
            for _ in 0..ranges {
                skip_varints(buf, 2)?; // gap, ack range length
            }
            if ty == 0x03 {
                skip_varints(buf, 3)?; // ECT0, ECT1, ECN-CE counts
            }
            return Some(Frame::Ack);
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
            varint(buf)?; // stream id
            if ty & 0x04 != 0 {
                varint(buf)?; // offset
            }
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
            return Some(Frame::Stream {
                len,
                fin: ty & 0x01 != 0,
            });
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
        // ACK_FREQUENCY
        0xaf => skip_varints(buf, 4)?,
        _ => return None,
    }
    Some(Frame::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
