use anyhow::{Result, anyhow};
use bytes::Bytes;
use msquic_async::msquic::{
    BufferRef, CertificateFile, Configuration, Credential, CredentialConfig, Registration,
    RegistrationConfig, Settings,
};
use std::future::poll_fn;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, trace};
use utils::{
    bin::{self, ServerArgs},
    perf::Blob,
};

const TARGET: &str = "msquic::server";

pub struct Server {
    args: ServerArgs,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        Ok(Server { args })
    }

    async fn listen(&mut self) -> Result<()> {
        let registration = Registration::new(&RegistrationConfig::default())?;
        let alpn = [BufferRef::from("perf")];
        let config = Configuration::open(
            &registration,
            &alpn,
            Some(
                &Settings::new()
                    .set_IdleTimeoutMs(10000)
                    .set_PeerBidiStreamCount(100)
                    .set_PeerUnidiStreamCount(100)
                    .set_DatagramReceiveEnabled()
                    .set_StreamMultiReceiveEnabled(),
            ),
        )?;

        let file = CertificateFile::new(self.args.key.clone(), self.args.cert.clone());
        let creds = Credential::CertificateFile(file);

        let cred_config = CredentialConfig::new().set_credential(creds);
        // .set_credential_flags(msquic::CredentialFlags::NO_CERTIFICATE_VALIDATION);
        config.load_credential(&cred_config)?;

        let listener = msquic_async::Listener::new(&registration, config)?;

        listener.start(&alpn, Some(self.args.listen))?;
        let server_addr = listener.local_addr()?;

        info!(target: TARGET, "Listening on {}", server_addr);

        // handle incoming connections and streams
        while let Ok(conn) = listener.accept().await {
            let fut = handle_request(conn);
            tokio::spawn(async move {
                if let Err(e) = fut.await {
                    error!(target: TARGET, "failed: {reason}", reason = e.to_string());
                }
            });
        }

        Ok(())
    }
}

async fn handle_request(conn: msquic_async::Connection) -> Result<(), anyhow::Error> {
    loop {
        error!("accepting inbound stream");
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            conn.accept_inbound_stream(),
        )
        .await
        {
            Ok(Ok(mut stream)) => {
                trace!(target: TARGET, "new stream id: {}", stream.id().expect("stream id"));

                let mut req = [0u8; 64 * 1024];
                stream.read(&mut req).await?;
                let blob = Blob::try_from(req.as_slice())
                    .map_err(|e| anyhow!("failed handling request: {}", e))?;
                let blob = Bytes::from_iter(blob);

                stream.write_all(&blob).await?;
                poll_fn(|cx| stream.poll_finish_write(cx)).await?;
            }
            Ok(Err(err)) => {
                error!(target: TARGET, "error on accept stream: {}", err);
                break;
            }
            Err(err) => {
                error!(target: TARGET, "timeout on accept stream: {}", err);
                break;
            }
        }
    }
    Ok(())
}
