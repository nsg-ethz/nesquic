//! This example demonstrates an HTTP server that serves files from a directory.
//!
//! Checkout the `README.md` for guidance.

use std::{
    ascii, fs,
    net::SocketAddr,
    path::PathBuf,
    str,
    sync::Arc,
};
use quinn_iut::noprotection::NoProtectionServerConfig;

use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use clap::Parser;

#[derive(Parser, Debug)]
#[clap(name = "server")]
struct Args {
    // do TLS handshake, but don't encrypt connection
    #[clap(long = "unencrypted")]
    unencrypted: bool,
    /// TLS private key in PEM format
    #[clap(short = 'k', long = "key", requires = "cert")]
    key: PathBuf,
    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert", requires = "key")]
    cert: PathBuf,
    /// Address to listen on
    #[clap(long = "listen", default_value = "[::1]:4433")]
    listen: SocketAddr,
}

pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

struct Blob {
    bit_size: u64,
    cursor: u64
}

impl Iterator for Blob {

    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor < self.bit_size {
            self.cursor += 8;
            Some(0)
        }   
        else {
            None
        }
    }

}

fn main() {
    let args = Args::parse();
    let code = {
        if let Err(e) = run(args) {
            eprintln!("ERROR: {e}");
            1
        } else {
            0
        }
    };
    std::process::exit(code);
}

#[tokio::main]
async fn run(args: Args) -> Result<()> {
    let cert_chain = fs::read(args.cert).context("failed to read certificate chain")?;
    let certs = vec![rustls::Certificate(cert_chain)];

    let key = fs::read(args.key).context("failed to read private key")?;
    let key = rustls::PrivateKey(key);

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    server_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

    let mut server_config = if args.unencrypted {
        quinn::ServerConfig::with_crypto(Arc::new(NoProtectionServerConfig::new(Arc::new(server_crypto))))
    }
    else {        
        quinn::ServerConfig::with_crypto(Arc::new(server_crypto))
    };
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    let endpoint = quinn::Endpoint::server(server_config, args.listen)?;
    println!("listening on {}", endpoint.local_addr()?);

    while let Some(conn) = endpoint.accept().await {
        println!("connection incoming");
        let fut = handle_connection(conn);
        tokio::spawn(async move {
            if let Err(e) = fut.await {
                eprintln!("connection failed: {reason}", reason = e.to_string())
            }
        });
    }

    Ok(())
}

async fn handle_connection(conn: quinn::Connecting) -> Result<()> {
    let connection = conn.await?;
    async {
        println!("established");

        // Each stream initiated by the client constitutes a new request.
        loop {
            let stream = connection.accept_bi().await;
            let stream = match stream {
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    println!("connection closed");
                    return Ok(());
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(s) => s,
            };
            let fut = handle_request(stream);
            tokio::spawn(
                async move {
                    if let Err(e) = fut.await {
                        eprintln!("failed: {reason}", reason = e.to_string());
                    }
                }
            );
        }
    }
    .await?;
    Ok(())
}

async fn handle_request(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
) -> Result<()> {
    let req = recv
        .read_to_end(64 * 1024)
        .await
        .map_err(|e| anyhow!("failed reading request: {}", e))?;
    let mut escaped = String::new();
    for &x in &req[..] {
        let part = ascii::escape_default(x).collect::<Vec<_>>();
        escaped.push_str(str::from_utf8(&part).unwrap());
    }
    
    // Execute the request
    let blob = process_get(&req)
        .map_err(|e| anyhow!("failed handling request: {}", e))?;

    // Write the response
    send.write_chunk(Bytes::from_iter(blob))
        .await
        .map_err(|e| anyhow!("failed to send response: {}", e))?;

    // Gracefully terminate the stream
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;
    
    println!("complete");
    Ok(())
}

fn parse_bit_size(value: &str) -> Result<u64> {
    if value.len() < 5 || !value.starts_with("/") || !value.ends_with("bit") {
        bail!("malformed blob size");
    }

    let bit_prefix = value
        .chars()
        .rev()
        .nth(3)
        .unwrap();
    if bit_prefix.to_digit(10) == None {
        let mult: u64 = match bit_prefix {
            'G' => 1024 * 1024 * 1024,
            'M' => 1024 * 1024,
            'K' => 1024,
            _ => bail!("unknown unit prefix")
        };

        let size = value[1..value.len()-4].parse::<u64>()?;
        Ok(mult * size)
    }
    else {
        let size = value[1..value.len()-3].parse::<u64>()?;
        Ok(size)
    }
} 

fn process_get(x: &[u8]) -> Result<Blob> {
    if x.len() < 4 || &x[0..4] != b"GET " {
        bail!("missing GET");
    }
    if x[4..].len() < 2 || &x[x.len() - 2..] != b"\r\n" {
        bail!("missing \\r\\n");
    }
    let x = &x[4..x.len() - 2];
    let end = x.iter().position(|&c| c == b' ').unwrap_or(x.len());
    let path = str::from_utf8(&x[..end]).context("path is malformed UTF-8")?;
    let bsize = parse_bit_size(path)?;

    Ok(Blob {
        bit_size: bsize,
        cursor: 0
    })
}