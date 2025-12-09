use crate::Benchmark;
use anyhow::{anyhow, bail, Result};
use std::net::ToSocketAddrs;
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
    send: Option<UnboundedSender<(u64, Request, oneshot::Sender<()>)>>,
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

        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
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

        let (benchmark, send) = Benchmark::new();
        let Ok(qconn) = quic::connect_with_config(socket, None, &params, benchmark).await else {
            bail!("Failed to establish connection");
        };

        self.conn = Some(qconn);
        self.send = Some(send);

        Ok(())
    }

    async fn run(&mut self) -> Result<()> {
        let Some(send) = self.send.as_mut() else {
            bail!("not connected");
        };

        let (tx, rx) = oneshot::channel();
        let request = Request::try_from(self.args.blob.clone())?;
        send.send((0, request, tx))?;

        rx.await?;

        Ok(())
    }
}
