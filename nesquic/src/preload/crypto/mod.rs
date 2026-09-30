//! Hooks for the AEAD calls that protect QUIC packets.
//!
//! QUIC seals every packet payload with the unprotected packet header as
//! associated data, so these calls are where packets and their frames become
//! visible. Each library reaches its AEAD through one of three APIs:
//!   - [`boringssl`]: BoringSSL's `EVP_AEAD_CTX_*` (quinn and noq via AWS-LC,
//!     quiche, ngtcp2, lsquic, xquic);
//!   - [`evp`]: OpenSSL's `EVP_CIPHER` API (msquic, picoquic via picotls,
//!     mvfst via fizz);
//!   - [`nss`]: NSS's experimental `SSL_AeadEncrypt`/`SSL_AeadDecrypt`
//!     (neqo).
//!
//! All of them need the crypto library to be linked dynamically, so the calls
//! go through the PLT and can be interposed.

use super::metrics::METRICS;
use super::{frame, parse_quic_header, qlog};

mod boringssl;
mod evp;
mod nss;

/// A cheap plausibility check that `ad` is a QUIC packet header rather than
/// some other use of the AEAD (e.g. session ticket encryption): long headers
/// carry at least flags, version and both CID lengths; short headers are
/// flags, a DCID of at most 20 bytes and a 1-4 byte packet number.
fn is_quic_header(ad: &[u8]) -> bool {
    match ad.first() {
        Some(first) if first & 0x80 != 0 => ad.len() >= 7,
        Some(_) => (2..=25).contains(&ad.len()),
        None => false,
    }
}

/// Records a packet with header `ad` and plaintext `payload`, `sent` by the
/// monitored process or received by it.
fn observe(ad: &[u8], payload: &[u8], sent: bool) {
    if !super::enabled() || !is_quic_header(ad) {
        return;
    }

    METRICS.record_packet(sent, &frame::summarize(payload));

    if qlog::enabled() {
        if let Some(header) = parse_quic_header(ad) {
            qlog::emit_packet(&header, payload.len(), sent);
        }
    }
}

/// [`observe`] for raw pointers as passed to the hooked C functions.
unsafe fn observe_raw(ad: *const u8, ad_len: usize, payload: *const u8, len: usize, sent: bool) {
    if ad.is_null() || payload.is_null() {
        return;
    }
    observe(
        std::slice::from_raw_parts(ad, ad_len),
        std::slice::from_raw_parts(payload, len),
        sent,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_quic_headers() {
        assert!(is_quic_header(&[0xc0, 0, 0, 0, 1, 0, 0]));
        assert!(!is_quic_header(&[0xc0, 0, 0, 0, 1]));
        assert!(is_quic_header(&[0x40, 0xaa, 0xbb, 0x01]));
        assert!(!is_quic_header(&[0x40; 26]));
        assert!(!is_quic_header(&[]));
    }
}
