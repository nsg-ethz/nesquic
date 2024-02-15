//! This example demonstrates an HTTP server that serves files from a directory.
//!
//! Checkout the `README.md` for guidance.

use std::{
    sync::Arc,
};
use quinn_iut::{
    bind_socket,
    load_certificates_from_pem,
    load_private_key_from_file,
    noprotection::NoProtectionServerConfig
};
use common::{
    perf::process_req,
    args::ServerArgs
};

use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use clap::Parser;

use log::{
    info,
    error
};
use quinn::TokioRuntime;

fn main() {
    env_logger::init();
    
    let args = ServerArgs::parse();
    let code = {
        if let Err(e) = run(args) {
            error!("ERROR: {e}");
            1
        } else {
            0
        }
    };
    std::process::exit(code);
}

#[tokio::main]
async fn run(args: ServerArgs) -> Result<()> {
    let certs = load_certificates_from_pem(args.cert.as_str()).context("failed to read certificate chain")?;
    let key = load_private_key_from_file(args.key.as_str()).context("failed to read private key")?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    server_crypto.alpn_protocols = vec![b"perf".to_vec()];

    let mut server_config = if args.unencrypted {
        quinn::ServerConfig::with_crypto(Arc::new(NoProtectionServerConfig::new(Arc::new(server_crypto))))
    }
    else {        
        quinn::ServerConfig::with_crypto(Arc::new(server_crypto))
    };
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    let socket = bind_socket(args.listen)?;
    let endpoint = quinn::Endpoint::new(
        Default::default(),
        Some(server_config),
        socket,
        Arc::new(TokioRuntime),
    )
    .context("creating endpoint")?;

    info!("listening on {}", endpoint.local_addr()?);

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
            tokio::spawn(
                async move {
                    if let Err(e) = fut.await {
                        error!("failed: {reason}", reason = e.to_string());
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

    // Execute the request
    let blob = process_req(&req)
        .map_err(|e| anyhow!("failed handling request: {}", e))?;
    
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