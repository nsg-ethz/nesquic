//! Hooks for the BoringSSL AEAD functions.
//!
//! quiche (BoringSSL) and quinn and noq (AWS-LC, a BoringSSL fork, via
//! rustls' `aws_lc_rs` provider) seal and open every packet through the same
//! API:
//!   - `EVP_AEAD_CTX_seal_scatter` — encrypt (seal) the packet payload
//!   - `EVP_AEAD_CTX_open`         — decrypt (open) the packet payload
//!
//! ngtcp2, lsquic and xquic seal with `EVP_AEAD_CTX_seal` instead. BoringSSL
//! implements it through the AEAD's method table rather than by calling
//! `EVP_AEAD_CTX_seal_scatter`, so hooking both never counts a packet twice.
//!
//! AWS-LC is built with `AWS_LC_SYS_NO_PREFIX` so it exports the plain
//! BoringSSL names (see docker/Dockerfile.quinn).

use super::observe_raw;
use super::METRICS;
use libc::{c_int, c_void};

redhook::hook! {
    unsafe fn EVP_AEAD_CTX_seal_scatter(
        ctx: *mut c_void, out: *mut u8, out_tag: *mut u8, out_tag_len: *mut usize,
        max_out_tag_len: usize, nonce: *const u8, nonce_len: usize,
        inp: *const u8, in_len: usize, extra_in: *const u8, extra_in_len: usize,
        ad: *const u8, ad_len: usize
    ) -> c_int => hook_seal_scatter {
        // The plaintext is only intact before sealing (which may happen in
        // place), so observe it first.
        observe_raw(ad, ad_len, inp, in_len, true);

        METRICS.time_crypto(|| redhook::real!(EVP_AEAD_CTX_seal_scatter)(
            ctx, out, out_tag, out_tag_len, max_out_tag_len, nonce, nonce_len,
            inp, in_len, extra_in, extra_in_len, ad, ad_len
        ))
    }
}

redhook::hook! {
    unsafe fn EVP_AEAD_CTX_seal(
        ctx: *const c_void, out: *mut u8, out_len: *mut usize, max_out_len: usize,
        nonce: *const u8, nonce_len: usize, inp: *const u8, in_len: usize,
        ad: *const u8, ad_len: usize
    ) -> c_int => hook_seal {
        observe_raw(ad, ad_len, inp, in_len, true);

        METRICS.time_crypto(|| redhook::real!(EVP_AEAD_CTX_seal)(
            ctx, out, out_len, max_out_len, nonce, nonce_len, inp, in_len, ad, ad_len
        ))
    }
}

redhook::hook! {
    unsafe fn EVP_AEAD_CTX_open(
        ctx: *const c_void, out: *mut u8, out_len: *mut usize, max_out_len: usize,
        nonce: *const u8, nonce_len: usize, inp: *const u8, in_len: usize,
        ad: *const u8, ad_len: usize
    ) -> c_int => hook_open {
        let ret = METRICS.time_crypto(|| redhook::real!(EVP_AEAD_CTX_open)(
            ctx, out, out_len, max_out_len, nonce, nonce_len, inp, in_len, ad, ad_len
        ));

        // The plaintext exists only once the packet was opened successfully.
        if ret == 1 && !out_len.is_null() {
            observe_raw(ad, ad_len, out, *out_len, false);
        }

        ret
    }
}
