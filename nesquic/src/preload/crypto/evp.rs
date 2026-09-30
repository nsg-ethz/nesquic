//! Hooks for OpenSSL's `EVP_CIPHER` API.
//!
//! msquic, picotls (picoquic) and fizz (mvfst) protect packets with an AEAD
//! `EVP_CIPHER_CTX` in a sequence of calls rather than a single one:
//!
//! ```text
//! EVP_{En,De}cryptInit_ex(ctx, NULL, NULL, NULL, nonce)
//! EVP_{En,De}cryptUpdate(ctx, NULL, .., header, len)   // AAD, 1+ calls
//! EVP_{En,De}cryptUpdate(ctx, out, .., in, len)        // payload, 1+ calls
//! EVP_{En,De}cryptFinal_ex(ctx, ..)                    // tag / verification
//! ```
//!
//! so the associated data and plaintext are collected per context and the
//! packet is observed once `Final` succeeds. Contexts that never pass
//! associated data (header protection, session ticket encryption) are
//! ignored.
//!
//! OpenSSL implements `EVP_CipherUpdate`/`EVP_CipherFinal_ex` and the
//! non-`_ex` finals by calling these functions through the PLT, so they are
//! covered as well; since the collected state is consumed by the first
//! `Final`, nested calls never observe a packet twice.

use std::cell::RefCell;
use std::collections::HashMap;

use libc::{c_int, c_uchar, c_void};

use super::observe;

/// Associated data beyond this is not a QUIC header (a long header with two
/// 20-byte CIDs and a large token is still well below).
const MAX_AD: usize = 4096;
/// Payloads beyond this are not QUIC packets.
const MAX_PAYLOAD: usize = 65536;

#[derive(Default)]
struct Op {
    ad: Vec<u8>,
    payload: Vec<u8>,
    /// Whether payload bytes were passed (after which new associated data
    /// starts a new operation).
    in_payload: bool,
    /// Whether the operation exceeded the limits and is not a QUIC packet.
    oversized: bool,
}

thread_local! {
    /// In-progress AEAD operations by `EVP_CIPHER_CTX` address. An operation
    /// runs on one thread from `Init` to `Final`.
    static OPS: RefCell<HashMap<usize, Op>> = RefCell::new(HashMap::new());
}

fn with_ops<R>(f: impl FnOnce(&mut HashMap<usize, Op>) -> R) -> Option<R> {
    OPS.try_with(|ops| ops.try_borrow_mut().ok().map(|mut ops| f(&mut ops)))
        .ok()
        .flatten()
}

fn reset(ctx: *mut c_void) {
    if super::super::enabled() {
        with_ops(|ops| ops.remove(&(ctx as usize)));
    }
}

/// Handles an `Update` call on `ctx` with input `data`: associated data if
/// `out` is null, payload otherwise. Payload bytes are only collected for
/// operations that started with associated data.
fn update(ctx: *mut c_void, out: *mut c_uchar, data: &[u8]) {
    with_ops(|ops| {
        let key = ctx as usize;
        if out.is_null() {
            let op = ops.entry(key).or_default();
            if op.in_payload {
                *op = Op::default();
            }
            op.oversized |= op.ad.len() + data.len() > MAX_AD;
            if !op.oversized {
                op.ad.extend_from_slice(data);
            }
        } else if let Some(op) = ops.get_mut(&key) {
            op.in_payload = true;
            op.oversized |= op.payload.len() + data.len() > MAX_PAYLOAD;
            if !op.oversized {
                op.payload.extend_from_slice(data);
            }
        }
    });
}

/// Handles the end of the operation on `ctx`, observing the packet if
/// `ok` (encrypted, or decrypted and authenticated).
fn finish(ctx: *mut c_void, ok: bool, sent: bool) {
    if let Some(Some(op)) = with_ops(|ops| ops.remove(&(ctx as usize))) {
        if ok && !op.oversized {
            observe(&op.ad, &op.payload, sent);
        }
    }
}

unsafe fn slice<'a>(ptr: *const c_uchar, len: c_int) -> &'a [u8] {
    match usize::try_from(len) {
        Ok(len) if !ptr.is_null() => std::slice::from_raw_parts(ptr, len),
        _ => &[],
    }
}

redhook::hook! {
    unsafe fn EVP_EncryptInit_ex(
        ctx: *mut c_void, cipher: *const c_void, engine: *mut c_void,
        key: *const c_uchar, iv: *const c_uchar
    ) -> c_int => hook_encrypt_init {
        reset(ctx);
        redhook::real!(EVP_EncryptInit_ex)(ctx, cipher, engine, key, iv)
    }
}

redhook::hook! {
    unsafe fn EVP_DecryptInit_ex(
        ctx: *mut c_void, cipher: *const c_void, engine: *mut c_void,
        key: *const c_uchar, iv: *const c_uchar
    ) -> c_int => hook_decrypt_init {
        reset(ctx);
        redhook::real!(EVP_DecryptInit_ex)(ctx, cipher, engine, key, iv)
    }
}

redhook::hook! {
    unsafe fn EVP_CipherInit_ex(
        ctx: *mut c_void, cipher: *const c_void, engine: *mut c_void,
        key: *const c_uchar, iv: *const c_uchar, enc: c_int
    ) -> c_int => hook_cipher_init {
        reset(ctx);
        redhook::real!(EVP_CipherInit_ex)(ctx, cipher, engine, key, iv, enc)
    }
}

redhook::hook! {
    unsafe fn EVP_EncryptUpdate(
        ctx: *mut c_void, out: *mut c_uchar, out_len: *mut c_int,
        inp: *const c_uchar, in_len: c_int
    ) -> c_int => hook_encrypt_update {
        // Encryption may happen in place: collect the plaintext first.
        if super::super::enabled() {
            update(ctx, out, slice(inp, in_len));
        }
        redhook::real!(EVP_EncryptUpdate)(ctx, out, out_len, inp, in_len)
    }
}

redhook::hook! {
    unsafe fn EVP_DecryptUpdate(
        ctx: *mut c_void, out: *mut c_uchar, out_len: *mut c_int,
        inp: *const c_uchar, in_len: c_int
    ) -> c_int => hook_decrypt_update {
        if !super::super::enabled() {
            return redhook::real!(EVP_DecryptUpdate)(ctx, out, out_len, inp, in_len);
        }
        if out.is_null() {
            update(ctx, out, slice(inp, in_len));
        }
        let ret = redhook::real!(EVP_DecryptUpdate)(ctx, out, out_len, inp, in_len);
        // The plaintext is only there after decryption; it is authenticated
        // by `Final`.
        if ret == 1 && !out.is_null() && !out_len.is_null() {
            update(ctx, out, slice(out, *out_len));
        }
        ret
    }
}

redhook::hook! {
    unsafe fn EVP_EncryptFinal_ex(
        ctx: *mut c_void, out: *mut c_uchar, out_len: *mut c_int
    ) -> c_int => hook_encrypt_final_ex {
        let ret = redhook::real!(EVP_EncryptFinal_ex)(ctx, out, out_len);
        if super::super::enabled() {
            finish(ctx, ret == 1, true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn EVP_DecryptFinal_ex(
        ctx: *mut c_void, out: *mut c_uchar, out_len: *mut c_int
    ) -> c_int => hook_decrypt_final_ex {
        let ret = redhook::real!(EVP_DecryptFinal_ex)(ctx, out, out_len);
        if super::super::enabled() {
            finish(ctx, ret == 1, false);
        }
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: [u8; 4] = [0x40, 0xaa, 0xbb, 0x01];

    fn ctx(n: usize) -> *mut c_void {
        n as *mut c_void
    }

    fn take(n: usize) -> Option<Op> {
        with_ops(|ops| ops.remove(&n)).flatten()
    }

    #[test]
    fn collects_ad_and_payload_chunks() {
        let out = [0u8; 1].as_ptr() as *mut c_uchar;
        update(ctx(1), std::ptr::null_mut(), &HEADER[..2]);
        update(ctx(1), std::ptr::null_mut(), &HEADER[2..]);
        update(ctx(1), out, &[1, 2]);
        update(ctx(1), out, &[3]);
        let op = take(1).unwrap();
        assert_eq!(op.ad, HEADER);
        assert_eq!(op.payload, [1, 2, 3]);
        assert!(!op.oversized);
    }

    #[test]
    fn ignores_payload_without_ad() {
        // e.g. header protection: an ECB context without associated data.
        let out = [0u8; 1].as_ptr() as *mut c_uchar;
        update(ctx(2), out, &[1, 2, 3]);
        assert!(take(2).is_none());
    }

    #[test]
    fn new_ad_after_payload_restarts() {
        let out = [0u8; 1].as_ptr() as *mut c_uchar;
        update(ctx(3), std::ptr::null_mut(), &[0xff; 8]);
        update(ctx(3), out, &[1]);
        update(ctx(3), std::ptr::null_mut(), &HEADER);
        let op = take(3).unwrap();
        assert_eq!(op.ad, HEADER);
        assert!(op.payload.is_empty());
    }

    #[test]
    fn flags_oversized_operations() {
        let out = [0u8; 1].as_ptr() as *mut c_uchar;
        update(ctx(4), std::ptr::null_mut(), &HEADER);
        update(ctx(4), out, &vec![0; MAX_PAYLOAD + 1]);
        assert!(take(4).unwrap().oversized);
    }
}
