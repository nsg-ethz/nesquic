use anyhow::{Result, bail};
use msquic_async::{
    Connection,
    msquic::{
        BufferRef, Configuration, CredentialConfig, Registration, RegistrationConfig, Settings,
    },
};
use std::future::poll_fn;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::trace;
use utils::{
    bin::{self, ClientArgs},
    perf::Request,
};

const TARGET: &str = "msquic::client";

pub struct Client {
    args: ClientArgs,
    conn: Option<Connection>,
    config: Configuration,
    registration: Registration,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
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

        let cred_config = CredentialConfig::new_client().set_ca_certificate_file(args.cert.clone());
        config.load_credential(&cred_config)?;

        Ok(Client {
            args,
            conn: None,
            config,
            registration,
        })
    }

    async fn connect(&mut self) -> Result<()> {
        let conn = msquic_async::Connection::new(&self.registration)?;
        conn.start(&self.config, self.args.url.host_str().unwrap(), 4433)
            .await?;

        trace!(target: TARGET, "connected");
        self.conn = Some(conn);

        Ok(())
    }

    async fn run(&mut self) -> Result<()> {
        let Some(conn) = self.conn.as_mut() else {
            bail!("not connected");
        };

        let request = Request::try_from(self.args.blob.clone())?;
        let mut stream = conn
            .open_outbound_stream(msquic_async::StreamType::Bidirectional, false)
            .await?;
        stream.write_all(&request.to_bytes()).await?;
        poll_fn(|cx| stream.poll_finish_write(cx)).await?;
        trace!(target: TARGET, "sent request");
        let mut buf = [0u8; 64 * 1024];
        let mut res_len = 0;
        while let Ok(len) = stream.read(&mut buf).await {
            res_len += len;
            if len == 0 {
                break;
            }
        }

        if request.size != res_len {
            bail!(
                "received blob size ({}B) different from requested blob size ({}B)",
                res_len,
                request.size
            )
        }

        Ok(())
    }
}
