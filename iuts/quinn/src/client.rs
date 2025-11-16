use crate::{bind_socket, load_certificates_from_pem, noprotection::NoProtectionClientConfig};
use anyhow::{anyhow, bail, Result};
use log::info;
use quinn::{ClientConfig, TokioRuntime};
use std::{net::ToSocketAddrs, sync::Arc};
use utils::{
    bin,
    bin::ClientArgs,
    perf::{create_req, parse_blob_size, Stats},
};

pub struct Client {
    args: ClientArgs,
    config: ClientConfig,
    stats: Stats,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        let certs = load_certificates_from_pem(args.cert.as_str())?;
        for cert in certs {
            roots.add(&cert)?;
        }

        let mut client_crypto = rustls::ClientConfig::builder()
            .with_cipher_suites(crate::PERF_CIPHER_SUITES)
            .with_safe_default_kx_groups()
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();
        client_crypto.alpn_protocols = vec![b"perf".to_vec()];

        let config = if args.unencrypted {
            quinn::ClientConfig::new(Arc::new(NoProtectionClientConfig::new(Arc::new(
                client_crypto,
            ))))
        } else {
            quinn::ClientConfig::new(Arc::new(client_crypto))
        };

        let size = parse_blob_size(&args.blob)?;
        let stats = Stats::new(size);

        Ok(Client {
            args,
            config,
            stats,
        })
    }

    async fn run(&mut self) -> Result<()> {
        let remote = (
            self.args.url.host_str().unwrap(),
            self.args.url.port().unwrap_or(4433),
        )
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

        let addr = "[::]:0".parse().unwrap();
        let socket = bind_socket(addr)?;
        let mut endpoint =
            quinn::Endpoint::new(Default::default(), None, socket, Arc::new(TokioRuntime))?;
        endpoint.set_default_client_config(self.config.clone());

        let request = create_req(&self.args.blob)?;
        let host = self
            .args
            .url
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

        self.stats.start_measurement();
        let resp = recv
            .read_to_end(usize::max_value())
            .await
            .map_err(|e| anyhow!("failed to read response: {}", e))?;

        info!("received response: {}B", resp.len());

        let (duration, throughput) = self.stats.stop_measurement()?;
        info!(
            "response received in {:?} - {:.2} Mbit/s",
            duration, throughput
        );

        conn.close(0u32.into(), b"done");
        info!("waiting for server to close connection...");

        // Give the server a fair chance to receive the close packet
        endpoint.wait_idle().await;

        let res_size = parse_blob_size(&self.args.blob)? as usize;
        if res_size != resp.len() {
            bail!(
                "received blob size ({}B) different from requested blob size ({}B)",
                resp.len(),
                res_size
            )
        }

        Ok(())
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
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
