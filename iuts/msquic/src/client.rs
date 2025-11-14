use anyhow::Result;
use async_trait::async_trait;
use log::info;
use std::{future::poll_fn, net::SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use utils::{
    bin::{self, ClientArgs},
    perf::{Stats, create_req, parse_blob_size},
};

pub struct Client {
    args: ClientArgs,
    local_addr: SocketAddr,
    stats: Stats,
}

#[async_trait]
impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        let local_addr = "0.0.0.0:0".parse()?;
        let size = parse_blob_size(&args.blob)?;
        let stats = Stats::new(size);

        Ok(Client {
            args,
            local_addr,
            stats,
        })
    }

    async fn run(&mut self) -> Result<()> {
        let registration = msquic::Registration::new(&msquic::RegistrationConfig::default())?;

        let alpn = [msquic::BufferRef::from("sample")];
        let configuration = msquic::Configuration::open(
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
        let cred_config = msquic::CredentialConfig::new_client()
            .set_credential_flags(msquic::CredentialFlags::NO_CERTIFICATE_VALIDATION);
        configuration.load_credential(&cred_config)?;

        let conn = msquic_async::Connection::new(&registration)?;
        conn.start(&configuration, "127.0.0.1", 4567).await?;

        let mut stream = conn
            .open_outbound_stream(msquic_async::StreamType::Bidirectional, false)
            .await?;
        stream.write_all("hello".as_bytes()).await?;
        poll_fn(|cx| stream.poll_finish_write(cx)).await?;
        let mut buf = [0u8; 1024];
        let len = stream.read(&mut buf).await?;
        info!("received: {}", String::from_utf8_lossy(&buf[0..len]));
        poll_fn(|cx| conn.poll_shutdown(cx, 0)).await?;

        Ok(())
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}
