use std::{
    cell::RefCell,
    collections::HashMap,
    io,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use neqo_common::{Datagram, Tos, event::Provider as _};
use neqo_crypto::{AllowZeroRtt, AntiReplay};
use neqo_transport::{
    ConnectionEvent, ConnectionIdGenerator, ConnectionParameters, Output,
    RandomConnectionIdGenerator, State, StreamId,
    server::{ConnectionRef, Server as NeqoServer},
};
use tracing::{error, info, trace};
use utils::{bin, bin::ServerArgs, perf::Blob};

use crate::{bind_tokio_socket, init_default_crypto_db};

const TARGET: &str = "neqo::server";

/// Zero-filled static buffer for response payload generation (32 KiB).
static ZERO_BUF: [u8; 32768] = [0u8; 32768];

/// Per-stream state: request accumulation buffer and response send progress.
struct StreamState {
    /// Accumulates incoming bytes until we have the full 8-byte request header.
    read_buf: Vec<u8>,
    /// Set once the request is parsed. Holds bytes remaining to send.
    write_remaining: Option<usize>,
}

impl StreamState {
    fn new() -> Self {
        StreamState {
            read_buf: Vec::new(),
            write_remaining: None,
        }
    }
}

pub struct Server {
    args: ServerArgs,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        init_default_crypto_db()?;
        Ok(Server { args })
    }

    async fn listen(&mut self) -> Result<()> {
        let (socket, local_addr) = bind_tokio_socket(self.args.listen)?;

        info!(target: TARGET, "listening on {local_addr}");

        let anti_replay =
            AntiReplay::new(Instant::now(), Duration::from_millis(10), 7, 14)
                .context("create anti-replay")?;

        let cid_gen: Rc<RefCell<dyn ConnectionIdGenerator>> =
            Rc::new(RefCell::new(RandomConnectionIdGenerator::new(8)));

        //TODO: fixme
        // let cert_nickname = self.args.key.as_str();
        let cert_nickname = "nesquic";
        let mut neqo_server = NeqoServer::new(
            Instant::now(),
            &[cert_nickname],
            &["perf"],
            anti_replay,
            Box::new(AllowZeroRtt {}),
            cid_gen,
            ConnectionParameters::default(),
        )
        .context("create neqo server")?;

        let mut stream_states: HashMap<StreamId, StreamState> = HashMap::new();
        let mut timeout: Option<Duration>;

        loop {
            // 1. Drain application events from all active connections (synchronous).
            process_all_events(&neqo_server, &mut stream_states);

            // 2. Drive output: send pending UDP datagrams.
            loop {
                match neqo_server.process_output(Instant::now()) {
                    Output::Datagram(d) => {
                        socket
                            .send_to(d.as_ref(), d.destination())
                            .await
                            .context("send datagram")?;
                    }
                    Output::Callback(dur) => {
                        timeout = Some(dur);
                        break;
                    }
                    Output::None => {
                        timeout = None;
                        break;
                    }
                }
            }

            // 3. If connections still have events, loop immediately.
            if neqo_server.has_active_connections() {
                continue;
            }

            // 4. Wait for a new UDP datagram or the neqo callback timer.
            let timeout_dur = timeout;
            tokio::select! {
                biased;
                result = socket.readable() => {
                    result.context("socket readable")?;
                    let mut recv_buf = vec![0u8; 65536];
                    match socket.try_recv_from(&mut recv_buf) {
                        Ok((n, src)) => {
                            recv_buf.truncate(n);
                            let dgram = Datagram::new(src, local_addr, Tos::default(), recv_buf);
                            // process() feeds input and returns at most one output datagram.
                            match neqo_server.process(std::iter::once(dgram), Instant::now()) {
                                Output::Datagram(d) => {
                                    socket
                                        .send_to(d.as_ref(), d.destination())
                                        .await
                                        .context("send datagram after input")?;
                                }
                                Output::Callback(_) | Output::None => {}
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(e) => return Err(anyhow!(e)).context("recv_from"),
                    }
                }
                _ = async {
                    match timeout_dur {
                        Some(dur) => tokio::time::sleep(dur).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    // Timer fired — loop to call process_output again.
                }
            }
        }
    }
}

/// Drain application-level events from every active connection.
///
/// All stream I/O calls (stream_recv, stream_send, stream_close_send) are synchronous.
/// UDP sending is deferred to the caller via `neqo_server.process_output()`.
fn process_all_events(
    server: &NeqoServer,
    stream_states: &mut HashMap<StreamId, StreamState>,
) {
    #[allow(clippy::mutable_key_type)]
    let active = server.active_connections();

    for conn_ref in &active {
        loop {
            // The RefMut from borrow_mut() is dropped after next_event() returns.
            let Some(event) = conn_ref.borrow_mut().next_event() else {
                break;
            };
            handle_event(event, conn_ref, stream_states);
        }
    }
}

/// Dispatch a single connection event to the appropriate handler.
fn handle_event(
    event: ConnectionEvent,
    conn_ref: &ConnectionRef,
    stream_states: &mut HashMap<StreamId, StreamState>,
) {
    match event {
        ConnectionEvent::NewStream { stream_id } => {
            trace!(target: TARGET, "new stream {:?}", stream_id);
            stream_states.insert(stream_id, StreamState::new());
        }

        ConnectionEvent::RecvStreamReadable { stream_id } => {
            read_request(conn_ref, stream_id, stream_states);
        }

        ConnectionEvent::SendStreamWritable { stream_id } => {
            if let Some(state) = stream_states.get_mut(&stream_id) {
                try_send_response(conn_ref, stream_id, state);
            }
        }

        ConnectionEvent::StateChange(State::Connected) => {
            // Send a session ticket to enable 0-RTT on future connections.
            if let Err(e) = conn_ref.borrow_mut().send_ticket(Instant::now(), b"") {
                trace!(target: TARGET, "send_ticket failed: {:?}", e);
            }
        }

        _ => {}
    }
}

/// Read available bytes from a client-initiated bidirectional stream and start
/// sending the response once the full 8-byte request header has arrived.
///
/// Does not remove entries from `stream_states`; stale entries are harmless.
fn read_request(
    conn_ref: &ConnectionRef,
    stream_id: StreamId,
    stream_states: &mut HashMap<StreamId, StreamState>,
) {
    if !stream_id.is_client_initiated() || !stream_id.is_bidi() {
        return;
    }

    // get_mut returns an &mut StreamState that borrows stream_states for the
    // duration of this function. We must not call stream_states.remove() here.
    let Some(state) = stream_states.get_mut(&stream_id) else {
        return;
    };

    let mut read_buf = [0u8; 32768];
    loop {
        // Each borrow_mut() creates a temporary RefMut that is dropped after
        // stream_recv() returns, so the next borrow in this loop is safe.
        let result = conn_ref
            .borrow_mut()
            .stream_recv(stream_id, &mut read_buf);

        let (n, fin) = match result {
            Ok(r) => r,
            Err(e) => {
                error!(target: TARGET, "stream_recv error: {:?}", e);
                return;
            }
        };

        if n > 0 {
            state.read_buf.extend_from_slice(&read_buf[..n]);
        }

        // Parse the 8-byte request header once we have enough data.
        if state.write_remaining.is_none() && state.read_buf.len() >= 8 {
            match Blob::try_from(state.read_buf.as_slice()) {
                Ok(blob) => {
                    trace!(
                        target: TARGET,
                        "serving {:?}: {}B",
                        stream_id,
                        blob.size
                    );
                    state.write_remaining = Some(blob.size);
                }
                Err(e) => {
                    error!(target: TARGET, "failed to parse request: {:?}", e);
                    return;
                }
            }
        }

        // Attempt to start sending response data.
        if state.write_remaining.is_some() {
            try_send_response(conn_ref, stream_id, state);
        }

        if fin || n == 0 {
            break;
        }
    }
}

/// Attempt to send response data on a stream, respecting QUIC flow control.
///
/// If `stream_send` returns 0 bytes written (flow-controlled), we stop and wait
/// for the next `SendStreamWritable` event before resuming.
fn try_send_response(
    conn_ref: &ConnectionRef,
    stream_id: StreamId,
    state: &mut StreamState,
) {
    let remaining = match state.write_remaining {
        Some(ref mut r) => r,
        None => return, // request not yet received or already finished
    };

    while *remaining > 0 {
        let chunk_size = (*remaining).min(ZERO_BUF.len());
        // Each borrow_mut() is a temporary RefMut dropped after stream_send() returns.
        match conn_ref
            .borrow_mut()
            .stream_send(stream_id, &ZERO_BUF[..chunk_size])
        {
            Ok(0) => {
                // Flow-controlled — wait for SendStreamWritable event.
                return;
            }
            Ok(n) => {
                *remaining -= n;
            }
            Err(e) => {
                trace!(
                    target: TARGET,
                    "stream_send error on {:?}: {:?}",
                    stream_id,
                    e
                );
                state.write_remaining = None;
                return;
            }
        }
    }

    // All bytes have been written; close the send side of the stream.
    trace!(target: TARGET, "stream {:?} complete", stream_id);
    if let Err(e) = conn_ref.borrow_mut().stream_close_send(stream_id) {
        trace!(
            target: TARGET,
            "stream_close_send error on {:?}: {:?}",
            stream_id,
            e
        );
    }
    state.write_remaining = None;
}
