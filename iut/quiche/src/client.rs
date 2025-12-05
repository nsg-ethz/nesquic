use crate::Benchmark;
use anyhow::{anyhow, bail, Result};
use std::net::{SocketAddr, ToSocketAddrs};
use tokio::sync::oneshot;
use tokio_quiche::{quic, socket::Socket as QuicSocket, ConnectionParams};
use tracing::trace;
use utils::{
    bin::{self, ClientArgs},
    perf::{Request, Stats},
};

const TARGET: &str = "quiche::client";

pub struct Client {
    args: ClientArgs,
    local_addr: SocketAddr,
    stats: Stats,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        let local_addr = "0.0.0.0:0".parse()?;

        Ok(Client {
            args,
            local_addr,
            stats: Stats::new(),
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

        trace!(target: TARGET, "connecting to {host} at {remote}");

        // TODO: here we have to set the CA's certificate
        let mut params = ConnectionParams::default();
        params.settings.alpn = vec![b"perf".to_vec()];

        let request = Request::try_from(self.args.blob.clone())?;

        let (done, stats) = oneshot::channel::<Stats>();
        let benchmark = Benchmark::new(Some(request), Some(done));

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
