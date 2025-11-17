use anyhow::Result;
use msquic::Configuration;
use std::{future::poll_fn, net::SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info};
use utils::{
    bin::{self, ClientArgs},
    perf::{Stats, create_req, parse_blob_size},
};

pub struct Client {
    args: ClientArgs,
    config: Configuration,
    registration: msquic::Registration,
    local_addr: SocketAddr,
    stats: Stats,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        let local_addr = "0.0.0.0:0".parse()?;
        let size = parse_blob_size(&args.blob)?;
        let stats = Stats::new(size);

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

        let cred_config =
            msquic::CredentialConfig::new_client().set_ca_certificate_file(args.cert.clone());
        config.load_credential(&cred_config)?;

        Ok(Client {
            args,
            config,
            registration,
            local_addr,
            stats,
        })
    }

    async fn run(&mut self) -> Result<()> {
        info!("Establishing connection");
        let conn = msquic_async::Connection::new(&self.registration)?;
        conn.start(&self.config, "127.0.0.1", 4433).await?;

        info!("connected");

        let request = create_req(&self.args.blob)?;

        let mut stream = conn
            .open_outbound_stream(msquic_async::StreamType::Bidirectional, false)
            .await?;
        stream.write_all(&request).await?;
        poll_fn(|cx| stream.poll_finish_write(cx)).await?;
        debug!("sent request");
        let mut buf = [0u8; 1024];

        self.stats.start_measurement();
        let _ = stream.read(&mut buf).await?;

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
