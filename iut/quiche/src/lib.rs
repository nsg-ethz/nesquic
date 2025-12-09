use anyhow::anyhow;
use bytes::{Buf, Bytes};
use std::{collections::HashMap, io::Cursor, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio_quiche::{
    metrics::Metrics,
    quic::{HandshakeInfo, QuicheConnection},
    ApplicationOverQuic, QuicResult,
};
use tracing::{error, trace};
use utils::perf::{Blob, Request};

mod client;
mod server;

pub use client::Client;
pub use server::Server;

struct Benchmark {
    buf: [u8; 16384],
    reqs: mpsc::UnboundedReceiver<(u64, Request, oneshot::Sender<()>)>,
    pending_req: HashMap<u64, oneshot::Sender<()>>,
    pending_res: HashMap<u64, Cursor<Bytes>>,
}

impl Benchmark {
    fn new() -> (
        Self,
        mpsc::UnboundedSender<(u64, Request, oneshot::Sender<()>)>,
    ) {
        let (req_tx, req_rx) = mpsc::unbounded_channel();

        let benchmark = Benchmark {
            buf: [0u8; 16384],
            reqs: req_rx,
            pending_req: HashMap::new(),
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
        trace!("wait for data");
        tokio::time::sleep(Duration::MAX).await;
        Ok(())
    }

    fn process_reads(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        let mut buf = [0u8; 16384];
        for stream in qconn.readable() {
            trace!("stream_recv({})", stream);
            while let Ok((_, done)) = qconn.stream_recv(stream, &mut buf) {
                if qconn.is_server() {
                    let blob = Blob::try_from(buf.as_ref())?;
                    trace!("Received request for {}B", blob.size);
                    self.pending_res
                        .insert(stream, Cursor::new(Bytes::from_iter(blob)));
                } else if done {
                    let Some(tx) = self.pending_req.remove(&stream) else {
                        error!("Got a result from an unknown stream");
                        return QuicResult::Err(anyhow!("Unknown stream").into_boxed_dyn_error());
                    };
                    tx.send(()).unwrap();
                }
            }
        }

        Ok(())
    }

    fn process_writes(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        if let Ok((stream, req, res)) = self.reqs.try_recv() {
            trace!("Writing request");
            self.pending_req.insert(stream, res);
            qconn.stream_send(stream, &req.to_bytes(), true)?;
        }

        let mut completed_responses = Vec::new();

        for (stream, res) in self.pending_res.iter_mut() {
            trace!("Writing response");

            if let Ok(len) = qconn.stream_send(*stream, res.chunk(), true) {
                res.advance(len);
                if !res.has_remaining() {
                    completed_responses.push(*stream);
                }
            }
        }

        for stream in completed_responses {
            self.pending_res.remove(&stream);
        }

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
