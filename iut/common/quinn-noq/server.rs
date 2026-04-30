use super::bind_socket;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use crate::backend::{ConnectionError, Endpoint, Incoming, QuicServerConfig, RecvStream, SendStream, ServerConfig, TokioRuntime};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::sync::Arc;
use tracing::{error, info, trace};
use utils::{bin, bin::ServerArgs, perf::Blob};

pub struct Server {
    args: ServerArgs,
    config: ServerConfig,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        let certs = CertificateDer::pem_file_iter(&args.cert)
            .context("failed to read PEM from certificate chain file")?
            .collect::<Result<_, _>>()
            .context("invalid PEM-encoded certificate")?;
        let key = PrivateKeyDer::from_pem_file(&args.key)
            .context("failed to read PEM from private key file")?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        server_crypto.alpn_protocols = vec![b"perf".to_vec()];

        let mut config = ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));

        if let Some(ref dir) = args.qlog {
            let transport = crate::setup_qlog_transport(dir, "server")?;
            config.transport_config(Arc::new(transport));
        }

        Ok(Server { args, config })
    }

    async fn listen(&mut self) -> Result<()> {
        let socket = bind_socket(self.args.listen)?;
        let endpoint = Endpoint::new(
            Default::default(),
            Some(self.config.clone()),
            socket,
            Arc::new(TokioRuntime),
        )
        .context("creating endpoint")?;

        info!(target: crate::SERVER_TARGET, "Listening on {}", endpoint.local_addr()?);

        while let Some(conn) = endpoint.accept().await {
            trace!(target: crate::SERVER_TARGET, "connection incoming");
            let fut = handle_connection(conn);
            tokio::spawn(async move {
                if let Err(e) = fut.await {
                    error!(target: crate::SERVER_TARGET, "connection failed: {reason}", reason = e.to_string())
                }
            });
        }

        Ok(())
    }
}

async fn handle_connection(conn: Incoming) -> Result<()> {
    let connection = conn.await?;
    async {
        trace!(target: crate::SERVER_TARGET, "established");

        loop {
            let stream = connection.accept_bi().await;
            trace!(target: crate::SERVER_TARGET, "stream accepted");

            let stream = match stream {
                Err(ConnectionError::ApplicationClosed { .. }) => {
                    trace!(target: crate::SERVER_TARGET, "connection closed");
                    return Ok(());
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(s) => s,
            };
            let fut = handle_request(stream);
            tokio::spawn(async move {
                if let Err(e) = fut.await {
                    error!(target: crate::SERVER_TARGET, "failed: {reason}", reason = e.to_string());
                }
            });
        }
    }
    .await?;
    Ok(())
}

async fn handle_request((mut send, mut recv): (SendStream, RecvStream)) -> Result<()> {
    let req = recv
        .read_to_end(64 * 1024)
        .await
        .map_err(|e| anyhow!("failed reading request: {}", e))?;

    let blob = Blob::try_from(req.as_slice()).map_err(|e| anyhow!("failed handling request: {}", e))?;
    trace!(target: crate::SERVER_TARGET, "serving {}", blob.size);

    send.write_chunk(Bytes::from_iter(blob))
        .await
        .map_err(|e| anyhow!("failed to send response: {}", e))?;

    send.finish()
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;

    trace!(target: crate::SERVER_TARGET, "complete");
    Ok(())
}
