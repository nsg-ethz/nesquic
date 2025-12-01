use anyhow::Result;
use msquic::{
    BufferRef, Configuration, CredentialConfig, Registration, RegistrationConfig, Settings,
};
use std::future::poll_fn;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info};
use utils::{
    bin::{self, ClientArgs},
    perf::{Request, Stats},
};

pub struct Client {
    args: ClientArgs,
    config: Configuration,
    registration: Registration,
    stats: Stats,
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
            config,
            registration,
            stats: Stats::new(),
        })
    }

    async fn run(&mut self) -> Result<()> {
        info!("Establishing connection");
        let conn = msquic_async::Connection::new(&self.registration)?;
        conn.start(&self.config, "127.0.0.1", 4433).await?;

        info!("connected");

        let request = Request::try_from(self.args.blob.clone())?;
        let mut stream = conn
            .open_outbound_stream(msquic_async::StreamType::Bidirectional, false)
            .await?;
        stream.write_all(&request.to_bytes()).await?;
        poll_fn(|cx| stream.poll_finish_write(cx)).await?;
        debug!("sent request");
        let mut buf = [0u8; 1024];

        self.stats.start_measurement();
        let len = stream.read(&mut buf).await?;
        self.stats.add_bytes(len)?;

        let (duration, throughput) = self.stats.stop_measurement()?;
        info!(
            "response received in {:?} - {:.2} Mbit/s",
            duration, throughput
        );

        poll_fn(|cx| conn.poll_shutdown(cx, 0)).await?;

        Ok(())
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}
