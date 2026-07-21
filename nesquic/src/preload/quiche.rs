//! Hooks for the boringssl functions quiche uses to protect QUIC traffic.
//!
//! quiche statically links boringssl and calls it on the packet hot path:
//!   - `EVP_AEAD_CTX_seal_scatter` — AEAD encrypt (seal) the packet payload
//!   - `EVP_AEAD_CTX_open`         — AEAD decrypt (open) the packet payload

use super::{arm_reporter, parse_quic_header, qlog};
use libc::{c_int, c_void};

redhook::hook! {
    unsafe fn EVP_AEAD_CTX_seal_scatter(
        ctx: *mut c_void, out: *mut u8, out_tag: *mut u8, out_tag_len: *mut usize,
        max_out_tag_len: usize, nonce: *const u8, nonce_len: usize,
        inp: *const u8, in_len: usize, extra_in: *const u8, extra_in_len: usize,
        ad: *const u8, ad_len: usize
    ) -> c_int => hook_seal_scatter {
        arm_reporter();

        // `ad` is the unprotected QUIC header (AEAD associated data),
        // observed here before header protection masks the packet number.
        let header = std::slice::from_raw_parts(ad, ad_len);
        if let Some(header) = parse_quic_header(header) {
            qlog::emit_packet(&header, in_len, true);
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
        arm_reporter();

        // As in `hook_seal_scatter`, `ad` is the unprotected QUIC header.
        let header = std::slice::from_raw_parts(ad, ad_len);
        if let Some(header) = parse_quic_header(header) {
            qlog::emit_packet(&header, in_len, false);
        }

        redhook::real!(EVP_AEAD_CTX_open)(
            ctx, out, out_len, max_out_len, nonce, nonce_len, inp, in_len, ad, ad_len
        )
    }
}
