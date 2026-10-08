use crate::{settings, Benchmark, PendingRequest};
use anyhow::{anyhow, bail, Result};
use common::{bind_socket, load};
use std::{net::ToSocketAddrs, time::Duration};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_quiche::{quic, socket::Socket as QuicSocket, ConnectionParams, QuicConnection};
use tracing::trace;
use utils::{
    bin::{self, ClientArgs},
    perf::Request,
};

const TARGET: &str = "quiche::client";

pub struct Client {
    args: ClientArgs,
    conn: Option<QuicConnection>,
    send: Option<UnboundedSender<PendingRequest>>,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        Ok(Client {
            args,
            conn: None,
            send: None,
        })
    }

    async fn connect(&mut self) -> Result<()> {
        let remote = (
            self.args.url.host_str().unwrap(),
            self.args.url.port().unwrap_or(4433),
        )
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

        let local = if remote.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = tokio::net::UdpSocket::from_std(bind_socket(local.parse().unwrap())?)?;
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
        params.settings = settings();

        let (benchmark, send) = Benchmark::new();
        let Ok(qconn) = quic::connect_with_config(socket, None, &params, benchmark).await else {
            bail!("Failed to establish connection");
        };

        self.conn = Some(qconn);
        self.send = Some(send);

        Ok(())
    }

    async fn run(&mut self) -> Result<Vec<Duration>> {
        let Some(send) = self.send.as_ref() else {
            bail!("not connected");
        };
        let blob = &self.args.blob;

        load(&self.args, || async move {
            let (tx, rx) = oneshot::channel();
            let request = Request::try_from(blob.clone())?;
            let size = request.len();
            send.send((request, tx))
                .map_err(|_| anyhow!("connection closed"))?;

            let received = rx.await?;
            if received != size {
                bail!(
                    "received blob size ({}B) different from requested blob size ({}B)",
                    received,
                    size
                )
            }

            Ok(())
        })
        .await
    }
}
