//! Reassembles the CRYPTO streams of the Initial and Handshake packets and
//! reads the cipher suite (ServerHello) and the QUIC transport parameters
//! (ClientHello, EncryptedExtensions) from the TLS handshake in them.

use std::collections::BTreeMap;

use qlog::events::quic::{TransportParametersSet, UnknownTransportParameter};
use s2n_codec::DecoderBuffer;
use s2n_quic_core::varint::VarInt;
use serde_json::{json, Map, Value};

use super::hex;

/// Bytes of a CRYPTO stream kept at most: the messages read here come first,
/// the certificate chain after them is not needed.
const MAX_LEN: usize = 1 << 16;

const CLIENT_HELLO: u8 = 1;
const SERVER_HELLO: u8 = 2;
const ENCRYPTED_EXTENSIONS: u8 = 8;
const QUIC_TRANSPORT_PARAMETERS: u16 = 0x39;
/// The codepoint of the drafts, which mvfst's own versions still use.
const QUIC_TRANSPORT_PARAMETERS_DRAFT: u16 = 0xffa5;

/// What a handshake message reveals. `extra` holds the transport
/// parameters that qlog 0.15 has no field for, by name.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Parameters {
    pub set: TransportParametersSet,
    pub extra: Map<String, Value>,
}

/// The CRYPTO stream of one packet number space and direction.
#[derive(Default)]
pub(crate) struct CryptoStream {
    data: Vec<u8>,
    /// CRYPTO frames that arrived ahead of `data`, by offset.
    pending: BTreeMap<u64, Vec<u8>>,
    /// Bytes of `data` that are complete handshake messages already read.
    parsed: usize,
}

impl CryptoStream {
    /// Adds the CRYPTO frame data `data` at `offset` and returns what the
    /// handshake messages it completes reveal.
    pub(crate) fn push(&mut self, offset: u64, data: &[u8]) -> Vec<Parameters> {
        if self.data.len() >= MAX_LEN {
            return Vec::new();
        }
        self.pending.insert(offset, data.to_vec());
        while let Some(entry) = self.pending.first_entry() {
            let Ok(start) = usize::try_from(*entry.key()) else {
                break;
            };
            if start > self.data.len() {
                break;
            }
            // Retransmissions overlap with what is already there.
            let chunk = entry.remove();
            if let Some(new) = chunk.get(self.data.len() - start..) {
                self.data.extend_from_slice(new);
            }
        }

        let mut found = Vec::new();
        while let [ty, l0, l1, l2, rest @ ..] = &self.data[self.parsed..] {
            let len = u32::from_be_bytes([0, *l0, *l1, *l2]) as usize;
            let Some(body) = rest.get(..len) else {
                break;
            };
            let params = match *ty {
                CLIENT_HELLO => client_hello(body),
                SERVER_HELLO => server_hello(body),
                ENCRYPTED_EXTENSIONS => extensions(DecoderBuffer::new(body)),
                _ => None,
            };
            found.extend(params);
            self.parsed += 4 + len;
        }
        found
    }
}

fn client_hello(body: &[u8]) -> Option<Parameters> {
    // legacy_version, random
    let buf = DecoderBuffer::new(body).skip(2 + 32).ok()?;
    let buf = buf.skip_with_len_prefix::<u8>().ok()?; // legacy_session_id
    let buf = buf.skip_with_len_prefix::<u16>().ok()?; // cipher_suites
    let buf = buf.skip_with_len_prefix::<u8>().ok()?; // legacy_compression_methods
    extensions(buf)
}

fn server_hello(body: &[u8]) -> Option<Parameters> {
    let buf = DecoderBuffer::new(body).skip(2 + 32).ok()?;
    let buf = buf.skip_with_len_prefix::<u8>().ok()?;
    let (suite, _) = buf.decode::<u16>().ok()?;
    let tls_cipher = match suite {
        0x1301 => "TLS_AES_128_GCM_SHA256".to_string(),
        0x1302 => "TLS_AES_256_GCM_SHA384".to_string(),
        0x1303 => "TLS_CHACHA20_POLY1305_SHA256".to_string(),
        _ => format!("{suite:#06x}"),
    };
    Some(Parameters {
        set: TransportParametersSet {
            tls_cipher: Some(tls_cipher),
            ..Default::default()
        },
        extra: Map::new(),
    })
}

/// The transport parameters in a TLS extension list.
fn extensions(buf: DecoderBuffer) -> Option<Parameters> {
    let (mut buf, _) = buf.decode_slice_with_len_prefix::<u16>().ok()?;
    while !buf.is_empty() {
        let (ty, rest) = buf.decode::<u16>().ok()?;
        let (data, rest) = rest.decode_slice_with_len_prefix::<u16>().ok()?;
        if ty == QUIC_TRANSPORT_PARAMETERS || ty == QUIC_TRANSPORT_PARAMETERS_DRAFT {
            return transport_parameters(data.into_less_safe_slice());
        }
        buf = rest;
    }
    None
}

/// Parses the quic_transport_parameters extension (RFC 9000 section 18).
fn transport_parameters(buf: &[u8]) -> Option<Parameters> {
    let mut params = TransportParametersSet::default();
    let mut extra = Map::new();
    let mut buf = DecoderBuffer::new(buf);
    while !buf.is_empty() {
        let (id, rest) = buf.decode::<VarInt>().ok()?;
        let (value, rest) = rest.decode_slice_with_len_prefix::<VarInt>().ok()?;
        buf = rest;
        let value = value.into_less_safe_slice();
        let int = || {
            Some(
                DecoderBuffer::new(value)
                    .decode::<VarInt>()
                    .ok()?
                    .0
                    .as_u64(),
            )
        };
        match id.as_u64() {
            0x00 => params.original_destination_connection_id = Some(hex(value)),
            0x01 => params.max_idle_timeout = int(),
            0x02 => params.stateless_reset_token = Some(hex(value)),
            0x03 => params.max_udp_payload_size = int().and_then(|v| v.try_into().ok()),
            0x04 => params.initial_max_data = int(),
            0x05 => params.initial_max_stream_data_bidi_local = int(),
            0x06 => params.initial_max_stream_data_bidi_remote = int(),
            0x07 => params.initial_max_stream_data_uni = int(),
            0x08 => params.initial_max_streams_bidi = int(),
            0x09 => params.initial_max_streams_uni = int(),
            0x0a => params.ack_delay_exponent = int().and_then(|v| v.try_into().ok()),
            0x0b => params.max_ack_delay = int().and_then(|v| v.try_into().ok()),
            0x0c => params.disable_active_migration = Some(true),
            0x0e => params.active_connection_id_limit = int().and_then(|v| v.try_into().ok()),
            0x0f => params.initial_source_connection_id = Some(hex(value)),
            0x10 => params.retry_source_connection_id = Some(hex(value)),
            // RFC 9368
            0x11 => {
                let versions: Vec<String> = value.chunks(4).map(hex).collect();
                extra.insert(
                    "version_information".into(),
                    json!({
                        "chosen_version": versions.first(),
                        "available_versions": versions.get(1..),
                    }),
                );
            }
            // RFC 9221
            0x20 => {
                extra.insert("max_datagram_frame_size".into(), json!(int()));
            }
            // RFC 9287
            0x2ab2 => {
                extra.insert("grease_quic_bit".into(), json!(true));
            }
            // draft-ferrieuxhamchaoui-quic-lossbits
            0x1057 => {
                extra.insert("loss_bits".into(), json!(int()));
            }
            // draft-huitema-quic-ts
            0x7158 => {
                extra.insert("timestamps".into(), json!(int()));
            }
            // draft-ietf-quic-ack-frequency in microseconds, under the IDs
            // of its successive drafts.
            0xde1a | 0xff02de1a | 0xff04de1b => {
                extra.insert("min_ack_delay".into(), json!(int()));
            }
            id => params.unknown_parameters.push(UnknownTransportParameter {
                id,
                value: hex(value),
            }),
        }
    }
    Some(Parameters { set: params, extra })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handshake message of type `ty` with `body`.
    fn message(ty: u8, body: &[u8]) -> Vec<u8> {
        let len = (body.len() as u32).to_be_bytes();
        [&[ty, len[1], len[2], len[3]][..], body].concat()
    }

    fn encrypted_extensions() -> Vec<u8> {
        // max_idle_timeout 30000, ack_delay_exponent 8, disable_active_migration,
        // initial_source_connection_id aabb, a GREASE parameter,
        // grease_quic_bit, min_ack_delay 1000.
        let params = [
            0x01, 0x04, 0x80, 0x00, 0x75, 0x30, 0x0a, 0x01, 0x08, 0x0c, 0x00, 0x0f, 0x02, 0xaa,
            0xbb, 0x1b, 0x01, 0xff, 0x6a, 0xb2, 0x00, 0xc0, 0x00, 0x00, 0x00, 0xff, 0x04, 0xde,
            0x1b, 0x02, 0x43, 0xe8,
        ];
        let ext = [&[0x00, 0x39, 0x00, params.len() as u8][..], &params].concat();
        let alpn = [0x00, 0x10, 0x00, 0x03, 0x00, 0x01, b'h'];
        let list = [&alpn[..], &ext].concat();
        message(
            ENCRYPTED_EXTENSIONS,
            &[&[0x00, list.len() as u8][..], &list].concat(),
        )
    }

    #[test]
    fn reads_transport_parameters_in_any_order() {
        let msg = encrypted_extensions();
        let mut stream = CryptoStream::default();
        let (head, tail) = msg.split_at(10);
        assert!(stream.push(10, tail).is_empty());
        // A retransmission of the head, overlapping with nothing new.
        let found = stream.push(0, head);
        assert_eq!(
            found,
            [Parameters {
                set: TransportParametersSet {
                    max_idle_timeout: Some(30000),
                    ack_delay_exponent: Some(8),
                    disable_active_migration: Some(true),
                    initial_source_connection_id: Some("aabb".to_string()),
                    unknown_parameters: vec![UnknownTransportParameter {
                        id: 0x1b,
                        value: "ff".to_string(),
                    }],
                    ..Default::default()
                },
                extra: json!({"grease_quic_bit": true, "min_ack_delay": 1000})
                    .as_object()
                    .unwrap()
                    .clone(),
            }]
        );
        assert!(stream.push(0, &msg).is_empty());
    }

    #[test]
    fn reads_cipher_suite() {
        let body = [&[0x03, 0x03][..], &[0; 32], &[0x00, 0x13, 0x03, 0x00]].concat();
        let found = CryptoStream::default().push(0, &message(SERVER_HELLO, &body));
        assert_eq!(
            found[0].set.tls_cipher.as_deref(),
            Some("TLS_CHACHA20_POLY1305_SHA256")
        );
    }
}
