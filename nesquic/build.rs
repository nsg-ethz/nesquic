use libbpf_cargo::SkeletonBuilder;
use std::{env, ffi::OsStr, path::PathBuf};

fn main() {
    let manifest_dir =
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set in build script");
    let manifest_dir = PathBuf::from(&manifest_dir);

    let log_level = std::env::var("BPF_LOG")
        .or(std::env::var("RUST_LOG"))
        .map(|s| s.to_lowercase());
    let log_level = match log_level.as_deref() {
        Ok("debug") => 2,
        Ok("trace") => 2,
        Ok("info") => 1,
        Ok("warn") => 1,
        Ok("error") => 1,
        _ => 0,
    };
    println!("cargo:rerun-if-env-changed=RUST_LOG");
    println!("cargo:rerun-if-env-changed=BPF_LOG");

    let src_dir = PathBuf::from(&manifest_dir).join("src").join("metrics");

    let src = src_dir.clone().join("metrics.bpf.c");
    println!("cargo:rerun-if-changed={src:?}");
    let out = src_dir.clone().join("metrics.skel.rs");

    SkeletonBuilder::new()
        .source(&src)
        .clang_args([
            OsStr::new("-D"),
            OsStr::new(format!("LOG_LEVEL={log_level}").as_str()),
            OsStr::new("-I"),
            OsStr::new("../include"),
        ])
        .build_and_generate(&out)
        .unwrap();
}
