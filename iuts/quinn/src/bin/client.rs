use std::{fs,
          net::ToSocketAddrs,
          path::PathBuf,
          sync::Arc,
          time::{Duration, Instant}
};

use anyhow::{anyhow, Result};
use clap::Parser;
// use log::{info, error};
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    url: Url,

    #[clap(short = 'c', long = "cert")]
    cert: PathBuf,
}

pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

#[tokio::main]
async fn run(args: Args) -> Result<()> {
    let remote = (args.url.host_str().unwrap(), args.url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

    let mut roots = rustls::RootCertStore::empty();
    let cert = rustls::Certificate(fs::read(args.cert)?);
    roots.add(&cert)?;

    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();

    client_crypto.alpn_protocols = ALPN_QUIC_HTTP
        .iter()
        .map(|&x| x.into())
        .collect();

    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    let request = format!("GET {}\r\n", args.url.path());
    let start = Instant::now();
    let host = args.url
        .host_str()
        .ok_or_else(|| anyhow!("no hostname specified"))?;

    eprintln!("connecting to {host} at {remote}");
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("failed to connect: {}", e))?;
    eprintln!("connected at {:?}", start.elapsed());
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("failed to open stream: {}", e))?;

    send.write_all(request.as_bytes())
        .await
        .map_err(|e| anyhow!("failed to send request: {}", e))?;
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;
    let response_start = Instant::now();
    eprintln!("request sent at {:?}", response_start - start);
    let resp = recv
        .read_to_end(usize::max_value())
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;

    let duration = response_start.elapsed();
    eprintln!(
        "response received in {:?} - {} KiB/s",
        duration,
        resp.len() as f32 / (duration_secs(&duration) * 1024.0)
    );

    conn.close(0u32.into(), b"done");
    println!("Waiting for server...");

    // Give the server a fair chance to receive the close packet
    endpoint.wait_idle().await;

    Ok(())
}

fn duration_secs(x: &Duration) -> f32 {
    x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
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
