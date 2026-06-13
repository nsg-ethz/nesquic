//! LD_PRELOAD interposition library that monitors the crypto functions a QUIC
//! library uses to protect QUIC traffic.
//!
//! Build produces `libnesquic_preload.so`; run an IUT with
//! `LD_PRELOAD=…/libnesquic_preload.so` to collect per-process counters. A
//! summary is emitted at process exit to stderr, or to the file named by
//! `NESQUIC_PRELOAD_OUT` when that env var is set.

pub mod quiche;
pub mod quinn;

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

pub(crate) static SEAL_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static SEAL_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static OPEN_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static OPEN_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static AES_HP_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static CHACHA_HP_CALLS: AtomicU64 = AtomicU64::new(0);

static REPORTER: Once = Once::new();

/// Register the exit reporter the first time any crypto function is hooked, so
/// processes that never touch the monitored library produce no output.
pub(crate) fn arm_reporter() {
    REPORTER.call_once(|| unsafe {
        libc::atexit(report);
    });
}

extern "C" fn report() {
    let summary = format!(
        "nesquic_preload pid={} seal_calls={} seal_bytes={} open_calls={} open_bytes={} aes_hp_calls={} chacha_hp_calls={}\n",
        std::process::id(),
        SEAL_CALLS.load(Ordering::Relaxed),
        SEAL_BYTES.load(Ordering::Relaxed),
        OPEN_CALLS.load(Ordering::Relaxed),
        OPEN_BYTES.load(Ordering::Relaxed),
        AES_HP_CALLS.load(Ordering::Relaxed),
        CHACHA_HP_CALLS.load(Ordering::Relaxed),
    );

    if let Ok(path) = std::env::var("NESQUIC_PRELOAD_OUT") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(summary.as_bytes());
            return;
        }
    }
    let _ = std::io::stderr().write_all(summary.as_bytes());
}
