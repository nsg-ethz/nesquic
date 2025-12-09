use crate::bind_socket;
use anyhow::{anyhow, bail, Result};
use quinn::{crypto::rustls::QuicClientConfig, ClientConfig, Connection, TokioRuntime};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::{net::ToSocketAddrs, sync::Arc};
use tracing::trace;
use utils::{bin, bin::ClientArgs, perf::Request};

const TARGET: &str = "quinn::client";

pub struct Client {
    args: ClientArgs,
    conn: Option<Connection>,
    config: ClientConfig,
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

        Ok(Client {
            args,
            conn: None,
            config,
        })
    }

    async fn connect(&mut self) -> Result<()> {
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

        let host = self
            .args
            .url
            .host_str()
            .ok_or_else(|| anyhow!("no hostname specified"))?;

        let conn = endpoint
            .connect(remote, host)?
            .await
            .map_err(|e| anyhow!("failed to connect: {}", e))?;
        self.conn = Some(conn);

        trace!(target: TARGET, "connected");

        Ok(())
    }

    async fn run(&mut self) -> Result<()> {
        let Some(conn) = self.conn.as_mut() else {
            bail!("not connected");
        };

        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow!("failed to open stream: {}", e))?;

        trace!(target: TARGET, "sending request");

        let request = Request::try_from(self.args.blob.clone())?;
        send.write_all(&request.to_bytes())
            .await
            .map_err(|e| anyhow!("failed to send request: {}", e))?;
        send.finish()
            .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;

        let resp = recv
            .read_to_end(usize::max_value())
            .await
            .map_err(|e| anyhow!("failed to read response: {}", e))?;

        trace!(target: TARGET, "received response: {}B", resp.len());

        if request.size != resp.len() {
            bail!(
                "received blob size ({}B) different from requested blob size ({}B)",
                resp.len(),
                request.size
            )
        }

        Ok(())
    }
}
