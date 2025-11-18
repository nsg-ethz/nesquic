use anyhow::{anyhow, bail, Result};
use quiche::Config;
use std::{
    net::{SocketAddr, ToSocketAddrs},
    time::Duration,
};
use tokio::sync::oneshot;
use tokio_quiche::{
    quic::{self, HandshakeInfo, QuicheConnection},
    socket::Socket as QuicSocket,
    ApplicationOverQuic, ConnectionParams, QuicResult,
};
use tracing::info;
use utils::{
    bin::{self, ClientArgs},
    perf::{create_req, parse_blob_size, Stats},
};

pub struct Client {
    args: ClientArgs,
    local_addr: SocketAddr,
    stats: Stats,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        let mut config = Config::new(quiche::PROTOCOL_VERSION)?;
        config.load_cert_chain_from_pem_file(&args.cert)?;
        config.set_application_protos(&[b"perf"])?;

        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(1350);
        config.set_max_send_udp_payload_size(1350);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);
        config.enable_early_data();

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
        let remote = (
            self.args.url.host_str().unwrap(),
            self.args.url.port().unwrap_or(4433),
        )
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

        let socket = tokio::net::UdpSocket::bind(self.local_addr).await?;
        socket.connect(remote).await?;

        let socket = QuicSocket::try_from(socket)?;
        let host = self
            .args
            .url
            .host_str()
            .ok_or_else(|| anyhow!("no hostname specified"))?;

        info!("connecting to {host} at {remote}");

        let mut params = ConnectionParams::default();
        params.settings.alpn = vec![b"perf".to_vec()];

        let (done, stats) = oneshot::channel::<Stats>();
        let benchmark = Benchmark::new(self.args.clone(), done)?;

        let Ok(_) = quic::connect_with_config(socket, None, &params, benchmark).await else {
            bail!("Failed to establish connection");
        };

        self.stats = stats.await?;

        Ok(())
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

struct Benchmark {
    args: ClientArgs,
    buf: [u8; 8192],
    stats: Stats,
    done: Option<oneshot::Sender<Stats>>,
    num_reqs_sent: u64,
}

impl Benchmark {
    fn new(args: ClientArgs, done: oneshot::Sender<Stats>) -> Result<Self> {
        let size = parse_blob_size(&args.blob)?;
        let stats = Stats::new(size);

        Ok(Benchmark {
            args,
            buf: [0u8; 8192],
            stats,
            done: Some(done),
            num_reqs_sent: 0,
        })
    }
}

impl ApplicationOverQuic for Benchmark {
    fn on_conn_established(
        &mut self,
        _: &mut QuicheConnection,
        _: &HandshakeInfo,
    ) -> QuicResult<()> {
        Ok(())
    }

    fn should_act(&self) -> bool {
        true
    }

    fn buffer(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    async fn wait_for_data(&mut self, _: &mut QuicheConnection) -> QuicResult<()> {
        tokio::time::sleep(Duration::MAX).await;
        Ok(())
    }

    fn process_reads(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        let mut buf = [0u8; 8192];
        while let Some(stream) = qconn.stream_readable_next() {
            let (_, done) = qconn.stream_recv(stream, &mut buf)?;
            if done {
                self.stats.stop_measurement()?;

                qconn.close(true, 0, "Done".as_bytes())?;

                self.done.take().unwrap().send(self.stats.clone()).unwrap();
            }
        }

        Ok(())
    }

    fn process_writes(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        if self.num_reqs_sent == 0 {
            self.num_reqs_sent += 1;
            self.stats.start_measurement();
            let request = create_req(&self.args.blob)?;
            qconn.stream_send(0, &request, true)?;
        }

        Ok(())
    }
}
