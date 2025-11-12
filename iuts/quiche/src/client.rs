use crate::{Connection, ConnectionId};
use anyhow::bail;
use async_trait::async_trait;
use log::{error, info, warn};
use quiche::Config;
use ring::{hmac::Key, rand};
use std::collections::HashMap;
use utils::{
    bin,
    bin::ClientArgs,
    io::{
        Connection as IoConnection, ConnectionDecision, DatagramEvent, Endpoint, Result, SocketAddr,
    },
    perf::{parse_blob_size, Stats},
};

#[derive(Clone, Default)]
struct ConnectionData {
    to_send: usize,
    sent: usize,
}

pub struct Client {
    args: ClientArgs,
    local_addr: SocketAddr,
    connections: HashMap<ConnectionId, (Connection, ConnectionData)>,
    config: Config,
    conn_id_seed: Key,
    stats: Stats,
}

#[async_trait]
impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        let size = parse_blob_size(&args.blob)?;
        let stats = Stats::new(size);

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

        let rng = rand::SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();

        let local_addr = "127.0.0.1:0".parse()?;

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
        Ok(())
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}
