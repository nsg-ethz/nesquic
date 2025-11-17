use anyhow::Result;
use std::fs;

fn main() -> Result<()> {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();

    let path = format!("{}/../res/pem/key.pem", env!("CARGO_MANIFEST_DIR"));
    let key = cert.signing_key.serialize_pem();
    fs::write(&path, &key)?;

    let path = format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR"));
    let cert = cert.cert.pem();
    fs::write(&path, &cert)?;

    Ok(())
}
