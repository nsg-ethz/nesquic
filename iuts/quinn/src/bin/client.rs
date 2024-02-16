use std::{
    net::ToSocketAddrs,
    sync::Arc,
};
use quinn_iut::{
    bind_socket,
    load_certificates_from_pem,
    noprotection::NoProtectionClientConfig
};
use common::{
    args::ClientArgs,
    perf::{
        Stats,
        create_req, 
        parse_blob_size
    }
};

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use log::{
    info,
    error
};
use quinn::TokioRuntime;


#[tokio::main]
async fn run(args: &ClientArgs, stats: &mut Stats) -> Result<()> {
    let remote = (args.url.host_str().unwrap(), args.url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

    let mut roots = rustls::RootCertStore::empty();
    let certs = load_certificates_from_pem(args.cert.as_str())?;
    for cert in certs {
        roots.add(&cert)?;
    }

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
    let socket = bind_socket(addr)?;
    let mut endpoint = quinn::Endpoint::new(Default::default(), None, socket, Arc::new(TokioRuntime))?;
    endpoint.set_default_client_config(client_config);

    let request = create_req(&args.blob)?;
    let host = args.url
        .host_str()
        .ok_or_else(|| anyhow!("no hostname specified"))?;

    info!("connecting to {host} at {remote}");
    let conn = endpoint
        .connect(remote, host)?
        .await
        .map_err(|e| anyhow!("failed to connect: {}", e))?;

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("failed to open stream: {}", e))?;

    // TODO: check if we do this the most performant way
    send.write_all(&request)
        .await
        .map_err(|e| anyhow!("failed to send request: {}", e))?;
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;
    
    stats.start_measurement();
    let resp = recv
        .read_to_end(usize::max_value())
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;

    info!("received response: {}B", resp.len());

    let (duration, throughput) = stats.stop_measurement()?;
    info!("response received in {:?} - {:.2} Mbit/s", duration, throughput);

    conn.close(0u32.into(), b"done");
    info!("waiting for server to close connection...");

    // Give the server a fair chance to receive the close packet
    endpoint.wait_idle().await;

    let res_size = parse_blob_size(&args.blob)? as usize;
    if res_size != resp.len() {
        bail!("received blob size ({}B) different from requested blob size ({}B)", resp.len(), res_size)
    }

    Ok(())
}

fn main() {
    env_logger::init();
    
    let args = ClientArgs::parse();
    let blob_size = parse_blob_size(&args.blob).expect("didn't recognize blob size");
    let mut stats = Stats::new(blob_size);
    let mut code = 0;

    for _ in 0..args.reps {
        if let Err(e) = run(&args, &mut stats) {
            error!("ERROR: {e}");
            code = 1;
            break;
        }
    }

    println!("{}", stats.summary());
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
