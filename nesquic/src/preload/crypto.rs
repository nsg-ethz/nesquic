//! Hooks for the BoringSSL AEAD functions that protect QUIC packets.
//!
//! Both quiche (BoringSSL) and quinn (AWS-LC, a BoringSSL fork, via rustls'
//! `aws_lc_rs` provider) seal and open every packet through the same API:
//!   - `EVP_AEAD_CTX_seal_scatter` — encrypt (seal) the packet payload
//!   - `EVP_AEAD_CTX_open`         — decrypt (open) the packet payload
//!
//! The IUT images link both libraries dynamically so these calls go through
//! the PLT and can be interposed; AWS-LC is built with `AWS_LC_SYS_NO_PREFIX`
//! so it exports the plain BoringSSL names (see docker/Dockerfile.quinn).
//!
//! The associated data `ad` is the unprotected packet header and the
//! plaintext is the packet payload, so this is where packets and their
//! frames become visible.

use super::metrics::METRICS;
use super::{frame, parse_quic_header, qlog};
use libc::{c_int, c_void};
use std::sync::atomic::Ordering::Relaxed;

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

unsafe fn observe(ad: *const u8, ad_len: usize, payload: &[u8], sent: bool) {
    if !super::enabled() || ad.is_null() {
        return;
    }
    let ad = std::slice::from_raw_parts(ad, ad_len);
    if !is_quic_header(ad) {
        return;
    }

    let acks = frame::count_acks(payload);
    let (packets, ack_frames) = if sent {
        (&METRICS.packets_sent, &METRICS.acks_sent)
    } else {
        (&METRICS.packets_received, &METRICS.acks_received)
    };
    packets.fetch_add(1, Relaxed);
    if acks > 0 {
        ack_frames.fetch_add(acks, Relaxed);
    }

    if qlog::enabled() {
        if let Some(header) = parse_quic_header(ad) {
            qlog::emit_packet(&header, payload.len(), sent);
        }
    }
}

redhook::hook! {
    unsafe fn EVP_AEAD_CTX_seal_scatter(
        ctx: *mut c_void, out: *mut u8, out_tag: *mut u8, out_tag_len: *mut usize,
        max_out_tag_len: usize, nonce: *const u8, nonce_len: usize,
        inp: *const u8, in_len: usize, extra_in: *const u8, extra_in_len: usize,
        ad: *const u8, ad_len: usize
    ) -> c_int => hook_seal_scatter {
        // The plaintext is only intact before sealing (which may happen in
        // place), so observe it first.
        if !inp.is_null() {
            observe(ad, ad_len, std::slice::from_raw_parts(inp, in_len), true);
        }

        redhook::real!(EVP_AEAD_CTX_seal_scatter)(
            ctx, out, out_tag, out_tag_len, max_out_tag_len, nonce, nonce_len,
            inp, in_len, extra_in, extra_in_len, ad, ad_len
        )
    }
}

redhook::hook! {
    unsafe fn EVP_AEAD_CTX_open(
        ctx: *const c_void, out: *mut u8, out_len: *mut usize, max_out_len: usize,
        nonce: *const u8, nonce_len: usize, inp: *const u8, in_len: usize,
        ad: *const u8, ad_len: usize
    ) -> c_int => hook_open {
        let ret = redhook::real!(EVP_AEAD_CTX_open)(
            ctx, out, out_len, max_out_len, nonce, nonce_len, inp, in_len, ad, ad_len
        );

        // The plaintext exists only once the packet was opened successfully.
        if ret == 1 && !out.is_null() && !out_len.is_null() {
            observe(ad, ad_len, std::slice::from_raw_parts(out, *out_len), false);
        }

        ret
    }
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
