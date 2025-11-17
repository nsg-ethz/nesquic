use anyhow::Result;
use std::{future::poll_fn, net::SocketAddr, ops::Deref, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info};
use utils::{
    bin::{self, ServerArgs},
    perf::{Stats, parse_blob_size},
};

pub struct Listener {
    inner: msquic_async::Listener,
}

impl Listener {
    pub fn new(inner: msquic_async::Listener) -> Self {
        Self { inner }
    }
}

unsafe impl Send for Listener {}

impl Deref for Listener {
    type Target = msquic_async::Listener;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub struct Alpn {
    inner: [msquic::BufferRef; 1],
}

impl Alpn {
    pub fn new(inner: [msquic::BufferRef; 1]) -> Self {
        Self { inner }
    }
}

unsafe impl Send for Alpn {}

impl AsRef<[msquic::BufferRef]> for Alpn {
    fn as_ref(&self) -> &[msquic::BufferRef] {
        &self.inner
    }
}

pub struct Server {
    args: ServerArgs,
    alpn: Alpn,
    listener: Listener,
    local_addr: SocketAddr,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        let local_addr = "0.0.0.0:0".parse()?;

        let registration = msquic::Registration::new(&msquic::RegistrationConfig::default())?;
        let alpn = [msquic::BufferRef::from("perf")];
        let config = msquic::Configuration::open(
            &registration,
            &alpn,
            Some(
                &msquic::Settings::new()
                    .set_IdleTimeoutMs(10000)
                    .set_PeerBidiStreamCount(100)
                    .set_PeerUnidiStreamCount(100)
                    .set_DatagramReceiveEnabled()
                    .set_StreamMultiReceiveEnabled(),
            ),
        )?;

        let alpn = Alpn::new(alpn);

        let key = format!("{}/../../res/pem/key.pem", env!("CARGO_MANIFEST_DIR"));
        let file = msquic::CertificateFile::new(key, args.cert.clone());
        let creds = msquic::Credential::CertificateFile(file);

        let cred_config = msquic::CredentialConfig::new_client().set_credential(creds);
        // .set_credential_flags(msquic::CredentialFlags::NO_CERTIFICATE_VALIDATION);
        config.load_credential(&cred_config)?;

        let listener = msquic_async::Listener::new(&registration, config)?;
        let listener = Listener::new(listener);

        Ok(Server {
            args,
            alpn,
            listener,
            local_addr,
        })
    }

    async fn listen(&mut self) -> Result<()> {
        let addr: SocketAddr = "127.0.0.1:4433".parse()?;
        self.listener.start(&self.alpn, Some(addr))?;
        let server_addr = self.listener.local_addr()?;

        info!("Listening on {}", server_addr);

        // handle incoming connections and streams
        while let Ok(conn) = self.listener.accept().await {
            info!("new connection established");
            tokio::spawn(async move {
                loop {
                    match conn.accept_inbound_stream().await {
                        Ok(mut stream) => {
                            info!("new stream id: {}", stream.id().expect("stream id"));
                            let mut buf = [0u8; 1024];
                            let len = stream.read(&mut buf).await?;
                            info!(
                                "reading from stream: {}",
                                String::from_utf8_lossy(&buf[0..len])
                            );
                            stream.write_all(&buf[0..len]).await?;
                            poll_fn(|cx| stream.poll_finish_write(cx)).await?;
                            drop(stream);
                        }
                        Err(err) => {
                            error!("error on accept stream: {}", err);
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
