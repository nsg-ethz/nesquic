use std::{
    cell::RefCell,
    collections::HashMap,
    num::NonZeroUsize,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use neqo_common::{event::Provider as _, Datagram};
use neqo_transport::{
    server::{ConnectionRef, Server as NeqoServer},
    ConnectionEvent, ConnectionIdGenerator, OutputBatch, RandomConnectionIdGenerator, State,
    StreamId,
};
use neqo_udp::RecvBuf;
use nss::{agent::AllowZeroRtt, AntiReplay};
use tracing::{error, info, trace};
use utils::{
    bin,
    bin::ServerArgs,
    perf::{Blob, ZEROS},
};

use crate::{connection_parameters, init_default_crypto_db, UdpSocket};

const TARGET: &str = "neqo::server";

/// Per-stream state: request accumulation buffer and response send progress.
#[derive(Default)]
struct StreamState {
    /// The leading 8 request bytes received so far.
    read_buf: Vec<u8>,
    /// Set once the request is parsed. Holds bytes remaining to send.
    write_remaining: Option<usize>,
}

/// Stream IDs repeat across connections, so the connection is part of the key.
type StreamStates = HashMap<(ConnectionRef, StreamId), StreamState>;

pub struct Server {
    args: ServerArgs,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        init_default_crypto_db()?;
        Ok(Server { args })
    }

    async fn listen(&mut self) -> Result<()> {
        let (socket, local_addr) = UdpSocket::bind(self.args.listen)?;

        info!(target: TARGET, "listening on {local_addr}");

        let anti_replay = AntiReplay::new(Instant::now(), Duration::from_millis(10), 7, 14)
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
            connection_parameters(),
        )
        .context("create neqo server")?;

        let max_datagrams =
            NonZeroUsize::new(socket.max_gso_segments()).unwrap_or(NonZeroUsize::MIN);
        let mut recv_buf = RecvBuf::default();
        #[allow(clippy::mutable_key_type)]
        let mut stream_states = StreamStates::new();

        loop {
            // 1. Drain application events from all active connections (synchronous).
            process_all_events(&neqo_server, &mut stream_states);

            // 2. Drive output: send pending datagram batches (GSO-aware).
            let timeout = loop {
                match neqo_server.process_multiple(None::<Datagram>, Instant::now(), max_datagrams)
                {
                    OutputBatch::DatagramBatch(d) => {
                        socket.send(&d).await.context("send datagram")?;
                    }
                    OutputBatch::Callback(dur) => break Some(dur),
                    OutputBatch::None => break None,
                }
            };

            // 3. If connections still have events, loop immediately.
            if neqo_server.has_active_connections() {
                continue;
            }

            // 4. Wait for new UDP datagrams or the neqo callback timer.
            tokio::select! {
                biased;
                result = socket.readable() => {
                    result.context("socket readable")?;
                    // Drain all datagrams available right now (GRO may have batched several).
                    while let Some(dgrams) = socket.recv(local_addr, &mut recv_buf).context("recv")? {
                        // Input is processed first; a batch here is a reply to it.
                        if let OutputBatch::DatagramBatch(d) =
                            neqo_server.process_multiple(dgrams, Instant::now(), max_datagrams)
                        {
                            socket.send(&d).await.context("send datagram after input")?;
                        }
                    }
                }
                _ = async {
                    match timeout {
                        Some(dur) => tokio::time::sleep(dur).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    // Timer fired — loop to call process_multiple again.
                }
            }
        }
    }
}

/// Drain application-level events from every active connection.
///
/// All stream I/O calls (stream_recv, stream_send, stream_close_send) are synchronous.
/// UDP sending is deferred to the caller via `neqo_server.process_output()`.
#[allow(clippy::mutable_key_type)]
fn process_all_events(server: &NeqoServer, stream_states: &mut StreamStates) {
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
#[allow(clippy::mutable_key_type)]
fn handle_event(event: ConnectionEvent, conn_ref: &ConnectionRef, stream_states: &mut StreamStates) {
    match event {
        ConnectionEvent::NewStream { stream_id } => {
            trace!(target: TARGET, "new stream {:?}", stream_id);
            stream_states.insert((conn_ref.clone(), stream_id), StreamState::default());
        }

        ConnectionEvent::RecvStreamReadable { stream_id } => {
            read_request(conn_ref, stream_id, stream_states);
        }

        ConnectionEvent::SendStreamWritable { stream_id } => {
            if let Some(state) = stream_states.get_mut(&(conn_ref.clone(), stream_id)) {
                try_send_response(conn_ref, stream_id, state);
            }
        }

        ConnectionEvent::StateChange(
            State::Closing { .. } | State::Draining { .. } | State::Closed(_),
        ) => {
            stream_states.retain(|(conn, _), _| conn != conn_ref);
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
/// Entries of `stream_states` are removed when their connection closes.
#[allow(clippy::mutable_key_type)]
fn read_request(conn_ref: &ConnectionRef, stream_id: StreamId, stream_states: &mut StreamStates) {
    if !stream_id.is_client_initiated() || !stream_id.is_bidi() {
        return;
    }

    let Some(state) = stream_states.get_mut(&(conn_ref.clone(), stream_id)) else {
        return;
    };

    let mut read_buf = [0u8; 32768];
    loop {
        // Each borrow_mut() creates a temporary RefMut that is dropped after
        // stream_recv() returns, so the next borrow in this loop is safe.
        let result = conn_ref.borrow_mut().stream_recv(stream_id, &mut read_buf);

        let (n, fin) = match result {
            Ok(r) => r,
            Err(e) => {
                error!(target: TARGET, "stream_recv error: {:?}", e);
                return;
            }
        };

        // Only the leading 8 bytes are significant (docs/PROTOCOL.md).
        let significant = n.min(8 - state.read_buf.len());
        state.read_buf.extend_from_slice(&read_buf[..significant]);

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
fn try_send_response(conn_ref: &ConnectionRef, stream_id: StreamId, state: &mut StreamState) {
    let remaining = match state.write_remaining {
        Some(ref mut r) => r,
        None => return, // request not yet received or already finished
    };

    while *remaining > 0 {
        let chunk_size = (*remaining).min(ZEROS.len());
        // Each borrow_mut() is a temporary RefMut dropped after stream_send() returns.
        match conn_ref
            .borrow_mut()
            .stream_send(stream_id, &ZEROS[..chunk_size])
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
