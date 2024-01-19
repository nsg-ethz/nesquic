use std::{fs,
          net::ToSocketAddrs,
          path::PathBuf,
          sync::Arc,
          time::{Duration, Instant}
};
use quinn_iut::{
    bind_socket,
    noprotection::NoProtectionClientConfig
};

use anyhow::{anyhow, Result};
use clap::Parser;
use quinn::TokioRuntime;
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {    
    url: Url,
    /// do TLS handshake, but don't encrypt connection
    #[clap(long = "unencrypted")]
    unencrypted: bool,
    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert")]
    cert: PathBuf,
    /// Send buffer size in bytes
    #[clap(long, default_value = "2097152")]
    send_buffer_size: usize,
    /// Receive buffer size in bytes
    #[clap(long, default_value = "2097152")]
    recv_buffer_size: usize,
}

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
        .with_cipher_suites(quinn_iut::PERF_CIPHER_SUITES)
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"perf".to_vec()];

    let client_config = if args.unencrypted {
        quinn::ClientConfig::new(Arc::new(NoProtectionClientConfig::new(Arc::new(client_crypto))))
    }
    else {        
        quinn::ClientConfig::new(Arc::new(client_crypto))
    };
    
    let addr = "[::]:0".parse().unwrap();
    let socket = bind_socket(addr, args.send_buffer_size, args.recv_buffer_size)?;
    let mut endpoint = quinn::Endpoint::new(Default::default(), None, socket, Arc::new(TokioRuntime))?;
    endpoint.set_default_client_config(client_config);

    let request = format!("GET {}\r\n", args.url.path());
    let start = Instant::now();
    let host = args.url
        .host_str()
        .ok_or_else(|| anyhow!("no hostname specified"))?;

    println!("connecting to {host} at {remote}");
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("failed to connect: {}", e))?;

    println!("connected at {:?}", start.elapsed());
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
    println!("request sent at {:?}", response_start - start);
    let resp = recv
        .read_to_end(usize::max_value())
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;

    let duration = response_start.elapsed();
    println!(
        "response received in {:?} - {} MiB/s",
        duration,
        resp.len() as f32 / (duration_secs(&duration) * 1024.0 * 1024.0)
    );

    conn.close(0u32.into(), b"done");
    println!("waiting for server to close connection...");

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

struct SkipServerVerification;

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
