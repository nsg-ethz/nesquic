use crate::bind_socket;
use anyhow::{anyhow, bail, Result};
use log::info;
use quinn::{crypto::rustls::QuicClientConfig, ClientConfig, TokioRuntime};
use rustls::pki_types::{pem::PemObject, CertificateDer};
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
        roots.add(CertificateDer::from_pem_file(&args.cert)?)?;

        let mut client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        client_crypto.alpn_protocols = vec![b"perf".to_vec()];

        let config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_crypto)?));

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
