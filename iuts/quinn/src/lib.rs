use anyhow::{anyhow, Context, Result};
use rustls::{Certificate, PrivateKey};
use socket2::{Domain, Protocol, Socket, Type};
use std::{fs::File, io::BufReader, net::SocketAddr};

pub mod client;
pub mod noprotection;
pub mod server;

pub fn bind_socket(addr: SocketAddr) -> Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))
        .context("create socket")?;

    if addr.is_ipv6() {
        socket.set_only_v6(false).context("set_only_v6")?;
    }

    socket
        .bind(&socket2::SockAddr::from(addr))
        .context("binding endpoint")?;

    Ok(socket.into())
}

pub fn load_private_key_from_file(path: &str) -> Result<PrivateKey> {
    let file = File::open(&path)?;
    let mut reader = BufReader::new(file);
    let mut keys: Vec<_> = rustls_pemfile::ec_private_keys(&mut reader).collect();

    match keys.len() {
        0 => Err(anyhow!("No PKCS8-encoded private key found in {path}")),
        1 => Ok(PrivateKey(keys.remove(0)?.secret_sec1_der().to_owned())),
        _ => Err(anyhow!(
            "More than one PKCS8-encoded private key found in {path}"
        )),
    }
}

pub fn load_certificates_from_pem(path: &str) -> std::io::Result<Vec<Certificate>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader);

    certs
        .into_iter()
        .map(|c| c.and_then(|k| Ok(Certificate(k.as_ref().to_owned()))))
        .collect()
}

pub static PERF_CIPHER_SUITES: &[rustls::SupportedCipherSuite] = &[
    rustls::cipher_suite::TLS13_AES_128_GCM_SHA256,
    rustls::cipher_suite::TLS13_AES_256_GCM_SHA384,
    rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
];
