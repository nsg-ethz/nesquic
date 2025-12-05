use bytes::{Buf, Bytes};
use std::{io::Cursor, time::Duration};
use tokio::sync::oneshot;
use tokio_quiche::{
    metrics::Metrics,
    quic::{HandshakeInfo, QuicheConnection},
    ApplicationOverQuic, QuicResult,
};
use tracing::{error, trace, warn};
use utils::perf::{Blob, Request, Stats};

mod client;
mod server;

pub use client::Client;
pub use server::Server;

struct Benchmark {
    buf: [u8; 16384],
    req: Option<Request>,
    res: Option<Cursor<Bytes>>,
    stats: Stats,
    done: Option<oneshot::Sender<Stats>>,
}

impl Benchmark {
    fn new(req: Option<Request>, done: Option<oneshot::Sender<Stats>>) -> Self {
        Benchmark {
            buf: [0u8; 16384],
            req,
            res: None,
            stats: Stats::new(),
            done,
        }
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
            while let Ok((len, done)) = qconn.stream_recv(stream, &mut buf) {
                trace!(
                    "len: {}, is measuring: {}, done: {}",
                    len,
                    self.stats.is_measuring(),
                    done
                );

                if self.stats.is_measuring() {
                    self.stats.add_bytes(len)?;
                    if done {
                        trace!("Closing connection");
                        self.stats.stop_measurement()?;
                        qconn.close(true, 0, "Done".as_bytes())?;
                    }
                } else {
                    let blob = Blob::try_from(buf.as_ref())?;
                    trace!("Received request for {}B", blob.size);
                    self.res = Some(Cursor::new(Bytes::from_iter(blob)));
                }
            }
        }

        Ok(())
    }

    fn process_writes(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        if let Some(req) = self.req.take() {
            trace!("Writing request");
            self.stats.start_measurement();
            qconn.stream_send(0, &req.to_bytes(), true)?;
        }
        if let Some(ref mut res) = self.res {
            trace!("Writing response");

            if let Ok(len) = qconn.stream_send(0, res.chunk(), true) {
                res.advance(len);
                if !res.has_remaining() {
                    self.res.take();
                }
            }
        }

        Ok(())
    }

    fn on_conn_close<M: Metrics>(&mut self, _: &mut QuicheConnection, _: &M, res: &QuicResult<()>) {
        if let Err(e) = res {
            warn!("connection closed with error: {}", e);
            return;
        }

        trace!("connection closed");

        if let Some(done) = self.done.take() {
            if let Err(err) = done.send(self.stats.clone()) {
                error!("failed to send stats: {:?}", err);
            }
        }
    }
}
