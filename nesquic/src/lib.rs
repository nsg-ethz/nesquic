//! `libnesquic.so`: an `LD_PRELOAD` library that measures a QUIC
//! implementation under test from inside its own process and reports the
//! results to InfluxDB when the process exits. See [`preload`].

pub mod preload;
