use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_quiche::{
    metrics::Metrics,
    quic::{HandshakeInfo, QuicheConnection},
    quiche,
    settings::QuicSettings,
    ApplicationOverQuic, QuicResult,
};
use tracing::{error, trace};
use utils::perf::{Blob, Request, CONNECTION_WINDOW, IDLE_TIMEOUT, STREAM_WINDOW, ZEROS};

mod client;
mod server;

pub use client::Client;
pub use server::Server;

/// A request for a stream and the channel reporting the received bytes.
type PendingRequest = (u64, Request, oneshot::Sender<usize>);

fn settings() -> QuicSettings {
    let mut settings = QuicSettings::default();
    settings.alpn = vec![b"perf".to_vec()];
    settings.max_idle_timeout = Some(IDLE_TIMEOUT);
    settings.initial_max_data = CONNECTION_WINDOW.into();
    settings.max_connection_window = CONNECTION_WINDOW.into();
    settings.initial_max_stream_data_bidi_local = STREAM_WINDOW.into();
    settings.initial_max_stream_data_bidi_remote = STREAM_WINDOW.into();
    settings.max_stream_window = STREAM_WINDOW.into();
    // No stateless retry, like the other IUTs.
    settings.disable_client_ip_validation = true;
    settings
}

struct Benchmark {
    /// Also the GSO send buffer: tokio-quiche batches at most this many bytes.
    buf: Vec<u8>,
    reqs: mpsc::UnboundedReceiver<PendingRequest>,
    next_req: Option<PendingRequest>,
    /// Client: received bytes and the waiter per stream.
    pending_req: HashMap<u64, (usize, oneshot::Sender<usize>)>,
    /// Server: leading request bytes per stream.
    requests: HashMap<u64, Vec<u8>>,
    /// Server: response bytes left to send per stream.
    pending_res: HashMap<u64, usize>,
}

impl Benchmark {
    fn new() -> (Self, mpsc::UnboundedSender<PendingRequest>) {
        let (req_tx, req_rx) = mpsc::unbounded_channel();

        let benchmark = Benchmark {
            buf: vec![0u8; u16::MAX as usize],
            reqs: req_rx,
            next_req: None,
            pending_req: HashMap::new(),
            requests: HashMap::new(),
            pending_res: HashMap::new(),
        };

        (benchmark, req_tx)
    }
}

impl ApplicationOverQuic for Benchmark {
    fn on_conn_established(
        &mut self,
        _: &mut QuicheConnection,
        _: &HandshakeInfo,
    ) -> QuicResult<()> {
        trace!("Connection established");
        Ok(())
    }

    fn should_act(&self) -> bool {
        true
    }

    fn buffer(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    async fn wait_for_data(&mut self, _: &mut QuicheConnection) -> QuicResult<()> {
        match self.reqs.recv().await {
            Some(req) => self.next_req = Some(req),
            // Servers have no request sender: only packets drive them.
            None => std::future::pending().await,
        }
        Ok(())
    }

    fn process_reads(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        for stream in qconn.readable() {
            while let Ok((len, fin)) = qconn.stream_recv(stream, &mut self.buf) {
                if qconn.is_server() {
                    let req = self.requests.entry(stream).or_default();
                    let significant = len.min(8 - req.len());
                    req.extend_from_slice(&self.buf[..significant]);
                    if fin {
                        let blob = Blob::try_from(req.as_slice())?;
                        trace!("Received request for {}B", blob.size);
                        self.requests.remove(&stream);
                        self.pending_res.insert(stream, blob.size);
                    }
                    continue;
                }

                let Some((received, _)) = self.pending_req.get_mut(&stream) else {
                    error!("Got a result from an unknown stream");
                    continue;
                };
                *received += len;
                if fin {
                    let (received, tx) = self.pending_req.remove(&stream).unwrap();
                    let _ = tx.send(received);
                }
            }
        }

        Ok(())
    }

    fn process_writes(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        if let Some((stream, req, res)) = self.next_req.take() {
            trace!("Writing request");
            qconn.stream_send(stream, &req.to_bytes(), true)?;
            self.pending_req.insert(stream, (0, res));
        }

        self.pending_res.retain(|stream, remaining| loop {
            let len = (*remaining).min(ZEROS.len());
            match qconn.stream_send(*stream, &ZEROS[..len], *remaining == len) {
                Ok(sent) => {
                    *remaining -= sent;
                    if *remaining == 0 {
                        return false;
                    }
                    if sent < len {
                        return true;
                    }
                }
                Err(quiche::Error::Done) => return true,
                Err(e) => {
                    error!("failed to send response: {}", e);
                    return false;
                }
            }
        });

        Ok(())
    }

    fn on_conn_close<M: Metrics>(&mut self, _: &mut QuicheConnection, _: &M, res: &QuicResult<()>) {
        if let Err(e) = res {
            error!("connection closed with error: {}", e);
            return;
        }

        trace!("connection closed");
    }
}
