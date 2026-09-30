use anyhow::Result;
use std::fs;

fn main() -> Result<()> {
    // mahimahi picks the first free 10.0.0.x pair as MAHIMAHI_BASE, which is not
    // 10.0.0.1 when the host already uses that address.
    // ponytail: covers bases up to 10.0.0.16; extend the range if a host hits more.
    let mut subs = vec!["127.0.0.1".to_string()];
    for i in 1..=16 {
        subs.push(format!("10.0.0.{i}"));
    }
    let cert = rcgen::generate_simple_self_signed(subs)?;

    let path = format!("{}/../res/pem/key.pem", env!("CARGO_MANIFEST_DIR"));
    let key = cert.signing_key.serialize_pem();
    fs::write(&path, &key)?;

    let path = format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR"));
    let cert = cert.cert.pem();
    fs::write(&path, &cert)?;

    Ok(())
}
