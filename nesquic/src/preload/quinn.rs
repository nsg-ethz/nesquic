//! Hooks for the aws-lc functions quinn uses to protect QUIC traffic.
//!
//! quinn statically links aws-lc (via rustls' `aws_lc_rs` provider) and calls
//! it on the packet hot path:
//!   - `EVP_AEAD_CTX_seal_scatter` — AEAD encrypt (seal) the packet payload
//!   - `EVP_AEAD_CTX_open`         — AEAD decrypt (open) the packet payload
//!
//! aws-lc-sys prefixes all of its exported symbols with a version-specific
//! prefix (`aws_lc_<version>_`) to avoid collisions when statically linked,
//! so the hooks below target the prefixed names rather than the upstream
//! boringssl ones. The prefix is tied to the aws-lc-sys version pinned in
//! `Cargo.lock` (currently 0.41.0) and will need updating if that dependency
//! is bumped.

use super::{arm_reporter, parse_quic_header, qlog};
use libc::{c_int, c_void};

redhook::hook! {
    unsafe fn aws_lc_0_41_0_EVP_AEAD_CTX_seal_scatter(
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

        redhook::real!(aws_lc_0_41_0_EVP_AEAD_CTX_seal_scatter)(
            ctx, out, out_tag, out_tag_len, max_out_tag_len, nonce, nonce_len,
            inp, in_len, extra_in, extra_in_len, ad, ad_len
        )
    }
}

redhook::hook! {
    unsafe fn aws_lc_0_41_0_EVP_AEAD_CTX_open(
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

        redhook::real!(aws_lc_0_41_0_EVP_AEAD_CTX_open)(
            ctx, out, out_len, max_out_len, nonce, nonce_len, inp, in_len, ad, ad_len
        )
    }
}
