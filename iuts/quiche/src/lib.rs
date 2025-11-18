use bytes::Bytes;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_quiche::{
    quic::{HandshakeInfo, QuicheConnection},
    ApplicationOverQuic, QuicResult,
};
use tracing::{debug, trace};
use utils::perf::{Blob, Request, Stats};

mod client;
mod server;

pub use client::Client;
pub use server::Server;

struct Benchmark {
    buf: [u8; 8192],
    req: Option<Request>,
    res: Option<Blob>,
    stats: Stats,
    done: Option<oneshot::Sender<Stats>>,
}

impl Benchmark {
    fn new(req: Option<Request>, done: oneshot::Sender<Stats>) -> Self {
        Benchmark {
            buf: [0u8; 8192],
            req,
            res: None,
            stats: Stats::new(),
            done: Some(done),
        }
    }
}

impl ApplicationOverQuic for Benchmark {
    fn on_conn_established(
        &mut self,
        _: &mut QuicheConnection,
        _: &HandshakeInfo,
    ) -> QuicResult<()> {
        debug!("Connection established");
        Ok(())
    }

    fn should_act(&self) -> bool {
        true
    }

    fn buffer(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    async fn wait_for_data(&mut self, _: &mut QuicheConnection) -> QuicResult<()> {
        tokio::time::sleep(Duration::MAX).await;
        Ok(())
    }

    fn process_reads(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        let mut buf = [0u8; 8192];
        while let Some(stream) = qconn.stream_readable_next() {
            trace!("stream_recv({})", stream);
            let (len, done) = qconn.stream_recv(stream, &mut buf)?;

            trace!(
                "is measuring: {}, done: {}",
                self.stats.is_measuring(),
                done
            );

            if self.stats.is_measuring() {
                self.stats.add_bytes(len)?;
                if done {
                    self.stats.stop_measurement()?;
                    qconn.close(true, 0, "Done".as_bytes())?;

                    self.done.take().unwrap().send(self.stats.clone()).unwrap();
                }
            } else {
                let blob = Blob::try_from(buf.as_ref())?;
                debug!("Received request for {}B", blob.size);
                self.res = Some(blob);
            }
        }

        Ok(())
    }

    fn process_writes(&mut self, qconn: &mut QuicheConnection) -> QuicResult<()> {
        if let Some(req) = self.req.take() {
            self.stats.start_measurement();
            qconn.stream_send(0, &req.to_bytes(), true)?;
        }
        if let Some(res) = self.res.take() {
            let res = Bytes::from_iter(res);
            qconn.stream_send(0, &res, true)?;
        }

        Ok(())
    }
}
