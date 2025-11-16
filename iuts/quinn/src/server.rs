use crate::{
    bind_socket, load_certificates_from_pem, load_private_key_from_file,
    noprotection::NoProtectionServerConfig,
};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use log::{error, info};
use quinn::{ServerConfig, TokioRuntime};
use std::sync::Arc;
use utils::{bin, bin::ServerArgs, perf::process_req};

pub struct Server {
    args: ServerArgs,
    config: ServerConfig,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        let certs = load_certificates_from_pem(args.cert.as_str())
            .context("failed to read certificate chain")?;
        let key =
            load_private_key_from_file(args.key.as_str()).context("failed to read private key")?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        server_crypto.alpn_protocols = vec![b"perf".to_vec()];

        let config = if args.unencrypted {
            quinn::ServerConfig::with_crypto(Arc::new(NoProtectionServerConfig::new(Arc::new(
                server_crypto,
            ))))
        } else {
            quinn::ServerConfig::with_crypto(Arc::new(server_crypto))
        };

        Ok(Server { args, config })
    }

    async fn listen(&mut self) -> Result<()> {
        let socket = bind_socket(self.args.listen)?;
        let endpoint = quinn::Endpoint::new(
            Default::default(),
            Some(self.config.clone()),
            socket,
            Arc::new(TokioRuntime),
        )
        .context("creating endpoint")?;

        info!("Listening on {}", endpoint.local_addr()?);

        while let Some(conn) = endpoint.accept().await {
            info!("connection incoming");
            let fut = handle_connection(conn);
            tokio::spawn(async move {
                if let Err(e) = fut.await {
                    error!("connection failed: {reason}", reason = e.to_string())
                }
            });
        }

        Ok(())
    }
}

async fn handle_connection(conn: quinn::Connecting) -> Result<()> {
    let connection = conn.await?;
    async {
        info!("established");

        // Each stream initiated by the client constitutes a new request.
        loop {
            let stream = connection.accept_bi().await;
            let stream = match stream {
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    info!("connection closed");
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
                    error!("failed: {reason}", reason = e.to_string());
                }
            });
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

    // Execute the request
    let blob = process_req(&req).map_err(|e| anyhow!("failed handling request: {}", e))?;

    // Write the response
    send.write_chunk(Bytes::from_iter(blob))
        .await
        .map_err(|e| anyhow!("failed to send response: {}", e))?;

    // Gracefully terminate the stream
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;

    info!("complete");
    Ok(())
}
