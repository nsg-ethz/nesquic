use anyhow::Result;
use futures::StreamExt as _;
use tokio::sync::oneshot;
use tokio_quiche::{
    metrics::DefaultMetrics, quic::SimpleConnectionIdGenerator, settings::TlsCertificatePaths,
    ConnectionParams,
};
use utils::{
    bin::{self, ServerArgs},
    perf::Stats,
};

use crate::Benchmark;

pub struct Server {
    args: ServerArgs,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        Ok(Server { args })
    }

    async fn listen(&mut self) -> Result<()> {
        let socket = tokio::net::UdpSocket::bind(self.args.listen).await?;

        let mut params = ConnectionParams::default();
        params.tls_cert = Some(TlsCertificatePaths {
            cert: &self.args.cert,
            private_key: &self.args.key,
            kind: tokio_quiche::settings::CertificateKind::X509,
        });

        let mut listeners = tokio_quiche::listen(
            [socket],
            params,
            SimpleConnectionIdGenerator,
            DefaultMetrics,
        )?;
        let accept = &mut listeners[0];

        while let Some(conn) = accept.next().await {
            let (done, stats) = oneshot::channel::<Stats>();
            let benchmark = Benchmark::new(None, done);
            conn?.start(benchmark);

            stats.await?;
        }

        Ok(())
    }
}
