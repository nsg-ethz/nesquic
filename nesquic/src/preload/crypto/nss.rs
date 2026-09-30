//! Hooks for NSS's QUIC AEAD functions.
//!
//! neqo (via nss-rs) protects packets with `SSL_AeadEncrypt` and
//! `SSL_AeadDecrypt`. They are experimental NSS APIs, which are not linked
//! but looked up at runtime through `SSL_GetExperimentalAPI`, so that is the
//! function hooked here: for these two names it hands out wrappers around the
//! real functions.

use std::ffi::CStr;
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use libc::{c_char, c_int, c_uint, c_void};

use super::observe_raw;

/// `SSL_AeadEncrypt` and `SSL_AeadDecrypt`, which share a signature.
type AeadFn = unsafe extern "C" fn(
    ctx: *const c_void,
    counter: u64,
    aad: *const u8,
    aad_len: c_uint,
    input: *const u8,
    input_len: c_uint,
    output: *mut u8,
    output_len: *mut c_uint,
    max_output: c_uint,
) -> c_int;

/// `SECSuccess`.
const SEC_SUCCESS: c_int = 0;
/// `SECFailure`.
const SEC_FAILURE: c_int = -1;

static REAL_ENCRYPT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static REAL_DECRYPT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn real(slot: &AtomicPtr<c_void>) -> Option<AeadFn> {
    let f = slot.load(Relaxed);
    // SAFETY: only ever set to the non-null result of looking up the
    // function of this signature in NSS.
    (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, AeadFn>(f) })
}

unsafe extern "C" fn aead_encrypt(
    ctx: *const c_void,
    counter: u64,
    aad: *const u8,
    aad_len: c_uint,
    input: *const u8,
    input_len: c_uint,
    output: *mut u8,
    output_len: *mut c_uint,
    max_output: c_uint,
) -> c_int {
    let Some(f) = real(&REAL_ENCRYPT) else {
        return SEC_FAILURE;
    };
    // Encryption may happen in place: observe the plaintext first.
    observe_raw(aad, aad_len as usize, input, input_len as usize, true);
    f(
        ctx, counter, aad, aad_len, input, input_len, output, output_len, max_output,
    )
}

unsafe extern "C" fn aead_decrypt(
    ctx: *const c_void,
    counter: u64,
    aad: *const u8,
    aad_len: c_uint,
    input: *const u8,
    input_len: c_uint,
    output: *mut u8,
    output_len: *mut c_uint,
    max_output: c_uint,
) -> c_int {
    let Some(f) = real(&REAL_DECRYPT) else {
        return SEC_FAILURE;
    };
    let ret = f(
        ctx, counter, aad, aad_len, input, input_len, output, output_len, max_output,
    );
    if ret == SEC_SUCCESS && !output_len.is_null() {
        observe_raw(aad, aad_len as usize, output, *output_len as usize, false);
    }
    ret
}

redhook::hook! {
    unsafe fn SSL_GetExperimentalAPI(name: *const c_char) -> *mut c_void => hook_get_experimental_api {
        let f = redhook::real!(SSL_GetExperimentalAPI)(name);
        if f.is_null() || name.is_null() || !super::super::enabled() {
            return f;
        }
        match CStr::from_ptr(name).to_bytes() {
            b"SSL_AeadEncrypt" => {
                REAL_ENCRYPT.store(f, Relaxed);
                aead_encrypt as AeadFn as *mut c_void
            }
            b"SSL_AeadDecrypt" => {
                REAL_DECRYPT.store(f, Relaxed);
                aead_decrypt as AeadFn as *mut c_void
            }
            _ => f,
        }
    }
}
