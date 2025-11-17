use crate::{Connection, ConnectionData, ConnectionId};
use anyhow::{anyhow, bail, Result};
use quiche::Config;
use ring::{hmac::Key, rand::*};
use std::{
    collections::HashMap,
    net::{SocketAddr, ToSocketAddrs},
};
use tokio_quiche::{
    quic::{self, HandshakeInfo, QuicheConnection},
    socket::Socket as QuicSocket,
    ApplicationOverQuic, ConnectionParams, QuicResult,
};
use tracing::{error, info, trace, warn};
use utils::{
    bin::{self, ClientArgs},
    perf::{create_req, parse_blob_size, Stats},
};

pub struct Client {
    args: ClientArgs,
    local_addr: SocketAddr,
    connections: HashMap<ConnectionId, Connection>,
    config: Config,
    conn_id_seed: Key,
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

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();

        let local_addr = "0.0.0.0:0".parse()?;
        let size = parse_blob_size(&args.blob)?;
        let stats = Stats::new(size);

        Ok(Client {
            args,
            local_addr,
            connections: HashMap::new(),
            conn_id_seed,
            config,
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
        let remote = self.args.url.as_str();
        let mut params = ConnectionParams::default();
        params.settings.alpn = vec![b"perf".to_vec()];
        let benchmark = Benchmark::new(self.args.clone());
        let Ok(conn) = quic::connect_with_config(socket, None, &params, benchmark).await else {
            bail!("Failed to establish connection");
        };

        Ok(())
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

struct Benchmark {
    args: ClientArgs,
    buf: [u8; 8192],
}

impl Benchmark {
    fn new(args: ClientArgs) -> Self {
        Benchmark {
            args,
            buf: [0u8; 8192],
        }
    }
}

impl ApplicationOverQuic for Benchmark {
    fn on_conn_established(
        &mut self,
        qconn: &mut QuicheConnection,
        handshake_info: &HandshakeInfo,
    ) -> QuicResult<()> {
        trace!("on_conn_established");
        Ok(())
    }

    fn should_act(&self) -> bool {
        trace!("should_act");
        true
    }

    fn buffer(&mut self) -> &mut [u8] {
        trace!("buffer");
        &mut self.buf
    }

    async fn wait_for_data(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        trace!("wait_for_data");
        Ok(())
    }

    fn process_reads(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        trace!("process_reads");
        Ok(())
    }

    fn process_writes(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        trace!("process_writes");
        Ok(())
    }
}
