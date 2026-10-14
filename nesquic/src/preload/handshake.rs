//! Reads the `ack_delay_exponent` transport parameter (RFC 9000 section
//! 18.2) of both endpoints from the TLS handshake, so that ACK delays can be
//! scaled to time.
//!
//! The client's transport parameters are in its ClientHello, the first
//! message of its Initial CRYPTO stream; the server's are in its
//! EncryptedExtensions, the first message of its Handshake CRYPTO stream.
//! Both are reassembled from the CRYPTO frames observed by the crypto hooks.
//!
//! Only the first handshake of the process is read: all connections of an
//! IUT share its configuration.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use super::frame::varint;
use super::QuicPacketType;

/// The exponent assumed when the parameter is absent (or not seen yet).
const DEFAULT_EXPONENT: u8 = 3;
/// CRYPTO data buffered per stream before giving up on it.
const MAX_BUFFERED: usize = 64 * 1024;

const CLIENT_HELLO: u8 = 1;
const ENCRYPTED_EXTENSIONS: u8 = 8;
const QUIC_TRANSPORT_PARAMETERS: u16 = 0x39;
const ACK_DELAY_EXPONENT: u64 = 0x0a;

/// The client's and the server's `ack_delay_exponent`.
static EXPONENTS: [AtomicU8; 2] = [
    AtomicU8::new(DEFAULT_EXPONENT),
    AtomicU8::new(DEFAULT_EXPONENT),
];

/// The Initial and Handshake CRYPTO streams, each in both directions.
static STREAMS: Mutex<[Stream; 4]> = Mutex::new([const { Stream::new() }; 4]);

/// The `ack_delay_exponent` of the client (if `client`) or the server.
pub(crate) fn ack_delay_exponent(client: bool) -> u8 {
    EXPONENTS[!client as usize].load(Ordering::Relaxed)
}

/// Records the CRYPTO frame data `data` at `offset` of a packet of type `ty`,
/// `sent` by the monitored process or received by it.
pub(crate) fn record_crypto(ty: QuicPacketType, sent: bool, offset: u64, data: &[u8]) {
    let level = match ty {
        QuicPacketType::Initial => 0,
        QuicPacketType::Handshake => 2,
        _ => return,
    };
    let Ok(mut streams) = STREAMS.lock() else {
        return;
    };
    let stream = &mut streams[level + sent as usize];
    if let Some((msg_type, exponent)) = stream.insert(offset, data) {
        let client = match msg_type {
            CLIENT_HELLO => true,
            ENCRYPTED_EXTENSIONS => false,
            _ => return,
        };
        EXPONENTS[!client as usize].store(exponent, Ordering::Relaxed);
    }
}

/// A CRYPTO stream, reassembled until its first handshake message is
/// complete.
struct Stream {
    /// The stream's data from offset 0 on.
    data: Vec<u8>,
    /// Data received beyond `data`, by offset.
    pending: BTreeMap<u64, Vec<u8>>,
    done: bool,
}

impl Stream {
    const fn new() -> Self {
        Self {
            data: Vec::new(),
            pending: BTreeMap::new(),
            done: false,
        }
    }

    /// Adds `bytes` at `offset`; once the first handshake message is
    /// complete, returns its type and the `ack_delay_exponent` it carries.
    fn insert(&mut self, offset: u64, bytes: &[u8]) -> Option<(u8, u8)> {
        if self.done {
            return None;
        }
        self.pending.insert(offset, bytes.to_vec());
        while let Some(entry) = self.pending.first_entry() {
            let start = *entry.key();
            let end = self.data.len() as u64;
            if start > end {
                break;
            }
            let bytes = entry.remove();
            let skip = (end - start) as usize;
            if let Some(new) = bytes.get(skip..) {
                self.data.extend_from_slice(new);
            }
        }

        let buffered = self.data.len() + self.pending.values().map(Vec::len).sum::<usize>();
        let message = first_message(&self.data);
        if message.is_some() || buffered > MAX_BUFFERED {
            self.done = true;
            self.data = Vec::new();
            self.pending = BTreeMap::new();
        }
        message
    }
}

/// The type and `ack_delay_exponent` of the first handshake message in
/// `data`, once it is complete.
fn first_message(data: &[u8]) -> Option<(u8, u8)> {
    let (&msg_type, rest) = data.split_first()?;
    let len = u32::from_be_bytes([0, *rest.first()?, *rest.get(1)?, *rest.get(2)?]) as usize;
    let mut body = rest.get(3..3 + len)?;
    let params = match msg_type {
        CLIENT_HELLO => {
            // legacy_version, random, legacy_session_id, cipher_suites,
            // legacy_compression_methods
            take(&mut body, 2 + 32)?;
            take_prefixed(&mut body, 1)?;
            take_prefixed(&mut body, 2)?;
            take_prefixed(&mut body, 1)?;
            transport_parameters(body)
        }
        ENCRYPTED_EXTENSIONS => transport_parameters(body),
        _ => None,
    };
    // A complete message is never looked at again: fall back to the default
    // if it is malformed.
    let exponent = params
        .and_then(ack_delay_exponent_param)
        .unwrap_or(DEFAULT_EXPONENT);
    Some((msg_type, exponent))
}

/// The `quic_transport_parameters` extension in the extensions block `body`.
fn transport_parameters(mut body: &[u8]) -> Option<&[u8]> {
    let mut extensions = take_prefixed(&mut body, 2)?;
    while !extensions.is_empty() {
        let ty = u16::from_be_bytes(take(&mut extensions, 2)?.try_into().ok()?);
        let data = take_prefixed(&mut extensions, 2)?;
        if ty == QUIC_TRANSPORT_PARAMETERS {
            return Some(data);
        }
    }
    None
}

/// The `ack_delay_exponent` in the transport parameters `params`; `None` if
/// they are malformed or the exponent is out of range (above 20).
fn ack_delay_exponent_param(mut params: &[u8]) -> Option<u8> {
    while !params.is_empty() {
        let id = varint(&mut params)?;
        let len = varint(&mut params)?;
        let mut value = take(&mut params, usize::try_from(len).ok()?)?;
        if id == ACK_DELAY_EXPONENT {
            return u8::try_from(varint(&mut value)?).ok().filter(|&e| e <= 20);
        }
    }
    Some(DEFAULT_EXPONENT)
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    let (head, tail) = buf.split_at_checked(n)?;
    *buf = tail;
    Some(head)
}

/// Takes a block prefixed by its `len_bytes`-byte big-endian length.
fn take_prefixed<'a>(buf: &mut &'a [u8], len_bytes: usize) -> Option<&'a [u8]> {
    let len = take(buf, len_bytes)?
        .iter()
        .fold(0usize, |acc, &b| (acc << 8) | b as usize);
    take(buf, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handshake message of type `msg_type` with `body`.
    fn message(msg_type: u8, body: &[u8]) -> Vec<u8> {
        let len = (body.len() as u32).to_be_bytes();
        [&[msg_type], &len[1..], body].concat()
    }

    /// An extensions block with `extensions` as (type, data) pairs.
    fn extensions(extensions: &[(u16, &[u8])]) -> Vec<u8> {
        let mut block = Vec::new();
        for (ty, data) in extensions {
            block.extend_from_slice(&ty.to_be_bytes());
            block.extend_from_slice(&(data.len() as u16).to_be_bytes());
            block.extend_from_slice(data);
        }
        [&(block.len() as u16).to_be_bytes()[..], &block].concat()
    }

    fn client_hello(params: &[u8]) -> Vec<u8> {
        let mut body = vec![0x03, 0x03];
        body.extend_from_slice(&[0xab; 32]); // random
        body.extend_from_slice(&[0x00]); // legacy_session_id
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // cipher_suites
        body.extend_from_slice(&[0x01, 0x00]); // legacy_compression_methods
        body.extend(extensions(&[(0x00, b"sni"), (0x39, params)]));
        message(CLIENT_HELLO, &body)
    }

    // initial_max_data = 1000, ack_delay_exponent = 8, a GREASE parameter.
    const PARAMS: &[u8] = &[0x04, 0x02, 0x43, 0xe8, 0x0a, 0x01, 0x08, 0x5a, 0x00];

    #[test]
    fn reads_client_hello() {
        assert_eq!(
            first_message(&client_hello(PARAMS)),
            Some((CLIENT_HELLO, 8))
        );
        // Absent: the default.
        assert_eq!(
            first_message(&client_hello(&[0x04, 0x01, 0x10])),
            Some((CLIENT_HELLO, 3))
        );
        // Out of range: the default.
        assert_eq!(
            first_message(&client_hello(&[0x0a, 0x01, 0x15])),
            Some((CLIENT_HELLO, 3))
        );
    }

    #[test]
    fn reads_encrypted_extensions() {
        let ee = message(
            ENCRYPTED_EXTENSIONS,
            &extensions(&[(0x10, b"h3"), (0x39, PARAMS)]),
        );
        assert_eq!(first_message(&ee), Some((ENCRYPTED_EXTENSIONS, 8)));
        // Incomplete.
        assert_eq!(first_message(&ee[..ee.len() - 1]), None);
    }

    #[test]
    fn reassembles_out_of_order() {
        let hello = client_hello(PARAMS);
        let (a, rest) = hello.split_at(10);
        let (b, c) = rest.split_at(20);
        let mut stream = Stream::new();
        assert_eq!(stream.insert(30, c), None);
        assert_eq!(stream.insert(0, a), None);
        // Overlaps what is already there.
        assert_eq!(stream.insert(5, &hello[5..30]), Some((CLIENT_HELLO, 8)));
        assert_eq!(stream.insert(10, b), None);
    }

    #[test]
    fn records_per_endpoint() {
        let ee = message(
            ENCRYPTED_EXTENSIONS,
            &extensions(&[(0x39, &[0x0a, 0x01, 0x05])]),
        );
        record_crypto(QuicPacketType::Handshake, false, 0, &ee);
        record_crypto(QuicPacketType::Initial, true, 0, &client_hello(PARAMS));
        assert_eq!(
            (ack_delay_exponent(true), ack_delay_exponent(false)),
            (8, 5)
        );
    }
}
