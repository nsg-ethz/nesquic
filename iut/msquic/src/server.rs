use anyhow::Result;
use msquic_async::msquic::{
    BufferRef, CertificateFile, Configuration, Credential, CredentialConfig, Registration,
    RegistrationConfig, Settings,
};
use std::future::poll_fn;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, trace};
use utils::bin::{self, ServerArgs};

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
            tokio::spawn(async move {
                loop {
                    match conn.accept_inbound_stream().await {
                        Ok(mut stream) => {
                            trace!(target: TARGET, "new stream id: {}", stream.id().expect("stream id"));
                            let mut buf = [0u8; 1024];
                            let len = stream.read(&mut buf).await?;
                            trace!(target: TARGET,
                                "reading from stream: {}",
                                String::from_utf8_lossy(&buf[0..len])
                            );
                            stream.write_all(&buf[0..len]).await?;
                            poll_fn(|cx| stream.poll_finish_write(cx)).await?;
                            drop(stream);
                        }
                        Err(err) => {
                            error!(target: TARGET, "error on accept stream: {}", err);
                            break;
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            });
        }

        Ok(())
    }
}
