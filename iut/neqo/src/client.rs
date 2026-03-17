use std::{
    cell::RefCell,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    num::NonZeroUsize,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use neqo_common::event::Provider as _;
use neqo_crypto::AuthenticationStatus;
use neqo_transport::{
    Connection, ConnectionEvent, ConnectionIdGenerator, ConnectionParameters, OutputBatch,
    RandomConnectionIdGenerator, State, StreamType,
};
use neqo_udp::RecvBuf;
use tracing::trace;
use utils::{bin, bin::ClientArgs, perf::Request};

use crate::{UdpSocket, init_default_crypto_db};

const TARGET: &str = "neqo::client";

pub struct Client {
    args: ClientArgs,
    conn: Option<Connection>,
    socket: Option<UdpSocket>,
    local_addr: Option<SocketAddr>,
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        init_default_crypto_db()?;
        Ok(Client {
            args,
            conn: None,
            socket: None,
            local_addr: None,
        })
    }

    async fn connect(&mut self) -> Result<()> {
        let host = self
            .args
            .url
            .host_str()
            .ok_or_else(|| anyhow!("no hostname in URL"))?
            .to_owned();
        let port = self.args.url.port().unwrap_or(4433);

        let remote = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow!("couldn't resolve {host}:{port}"))?;

        let bind_addr: SocketAddr = match remote {
            SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        };
        let (socket, local_addr) = UdpSocket::bind(bind_addr)?;

        let cid_gen: Rc<RefCell<dyn ConnectionIdGenerator>> =
            Rc::new(RefCell::new(RandomConnectionIdGenerator::new(8)));

        let mut conn = Connection::new_client(
            host.as_str(),
            &["perf"],
            cid_gen,
            local_addr,
            remote,
            ConnectionParameters::default(),
            Instant::now(),
        )
        .context("create QUIC connection")?;

        trace!(target: TARGET, "connecting to {remote}");

        let mut connected = false;
        drive_until(&mut conn, &socket, local_addr, |conn: &mut Connection| {
            while let Some(event) = conn.next_event() {
                match event {
                    ConnectionEvent::AuthenticationNeeded => {
                        conn.authenticated(AuthenticationStatus::Ok, Instant::now());
                    }
                    ConnectionEvent::StateChange(State::Connected | State::Confirmed) => {
                        connected = true;
                    }
                    ConnectionEvent::StateChange(State::Closing { ref error, .. }
                    | State::Draining { ref error, .. }) => {
                        if error.is_error() {
                            bail!("connection closing with error: {:?}", error);
                        }
                    }
                    ConnectionEvent::StateChange(State::Closed(ref reason)) => {
                        if reason.is_error() {
                            bail!("connection closed with error: {:?}", reason);
                        }
                    }
                    _ => {}
                }
            }
            Ok(connected)
        })
        .await?;

        trace!(target: TARGET, "connected");

        self.conn = Some(conn);
        self.socket = Some(socket);
        self.local_addr = Some(local_addr);

        Ok(())
    }

    async fn run(&mut self) -> Result<()> {
        let conn = self.conn.as_mut().ok_or_else(|| anyhow!("not connected"))?;
        let socket = self.socket.as_ref().ok_or_else(|| anyhow!("not connected"))?;
        let local_addr = self.local_addr.ok_or_else(|| anyhow!("not connected"))?;

        let request = Request::try_from(self.args.blob.clone())?;
        trace!(target: TARGET, "requesting {}B", request.size);

        let stream_id = conn
            .stream_create(StreamType::BiDi)
            .context("create bidirectional stream")?;

        // Send the 8-byte request header.
        let req_bytes = request.to_bytes();
        let sent = conn
            .stream_send(stream_id, &req_bytes)
            .context("send request")?;
        if sent != 8 {
            bail!("only sent {sent} of 8 request bytes");
        }
        conn.stream_close_send(stream_id)
            .context("close send side")?;

        trace!(target: TARGET, "request sent on stream {:?}", stream_id);

        let request_size = request.size;
        let mut received: usize = 0;
        let mut response_done = false;
        let mut read_buf = vec![0u8; 32 * 1024];

        drive_until(conn, socket, local_addr, |conn| {
            while let Some(event) = conn.next_event() {
                match event {
                    ConnectionEvent::RecvStreamReadable { stream_id: sid }
                        if sid == stream_id =>
                    {
                        loop {
                            let (n, fin) = conn
                                .stream_recv(sid, &mut read_buf)
                                .context("stream_recv")?;
                            received += n;
                            if fin {
                                response_done = true;
                                break;
                            }
                            if n == 0 {
                                break;
                            }
                        }
                    }
                    ConnectionEvent::StateChange(State::Closed(ref reason))
                        if reason.is_error() =>
                    {
                        bail!("connection closed unexpectedly: {:?}", reason);
                    }
                    _ => {}
                }
            }
            Ok(response_done)
        })
        .await?;

        trace!(target: TARGET, "received {received}B");

        if received != request_size {
            bail!(
                "received blob size ({received}B) different from requested blob size ({request_size}B)"
            );
        }

        // Graceful close: drive the connection through Closing → Closed so the
        // peer receives the CONNECTION_CLOSE frame and we wait for acknowledgement.
        conn.close(Instant::now(), 0, "done");
        drive_until(conn, socket, local_addr, |conn| {
            while let Some(event) = conn.next_event() {
                if let ConnectionEvent::StateChange(State::Closed(ref reason)) = event {
                    if reason.is_error() {
                        bail!("connection closed with error during shutdown: {:?}", reason);
                    }
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .await?;

        Ok(())
    }
}

/// Drive the neqo `Connection` synchronous state machine from an async context.
///
/// On each iteration:
/// 1. Calls `done(conn)` to drain connection events and check for completion.
/// 2. Drains outgoing datagram batches via `process_multiple_output`, retrying
///    on `WouldBlock` (send backpressure).
/// 3. Waits for the next incoming UDP packet or the neqo callback timer, then
///    feeds all received datagrams to `process_multiple_input` in one call.
async fn drive_until<F>(
    conn: &mut Connection,
    socket: &UdpSocket,
    local_addr: SocketAddr,
    mut done: F,
) -> Result<()>
where
    F: FnMut(&mut Connection) -> Result<bool>,
{
    let max_datagrams =
        NonZeroUsize::new(socket.max_gso_segments()).unwrap_or(NonZeroUsize::MIN);
    let mut recv_buf = RecvBuf::default();
    let mut timeout: Option<Duration>;

    loop {
        // 1. Let the caller process events and decide if we're done.
        if done(conn)? {
            return Ok(());
        }

        // 2. Drain all outgoing datagram batches (GSO-aware).
        loop {
            match conn.process_multiple_output(Instant::now(), max_datagrams) {
                OutputBatch::DatagramBatch(d) => {
                    // Retry on WouldBlock to respect OS send-buffer backpressure.
                    loop {
                        match socket.send(&d) {
                            Ok(()) => break,
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                socket.writable().await.context("socket writable")?;
                            }
                            Err(e) => return Err(e).context("send UDP datagram"),
                        }
                    }
                }
                OutputBatch::Callback(dur) => {
                    timeout = Some(dur);
                    break;
                }
                OutputBatch::None => {
                    timeout = None;
                    break;
                }
            }
        }

        // 3. If new events were generated while draining output, loop without waiting.
        if conn.has_events() {
            continue;
        }

        // 4. Wait for the next UDP packet or the neqo-provided timer.
        let timeout_dur = timeout;
        tokio::select! {
            biased;
            // wait for socket to be readable, then read and process one datagram
            result = socket.readable() => {
                result.context("socket readable")?;
                // Drain all datagrams available right now (GRO may have batched several).
                while let Some(dgrams) = socket.recv(local_addr, &mut recv_buf).context("recv")? {
                    conn.process_multiple_input(dgrams, Instant::now());
                }
            }
            _ = async {
                match timeout_dur {
                    Some(dur) => tokio::time::sleep(dur).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Timer fired — loop back to call process_multiple_output again.
            }
        }
    }
}
