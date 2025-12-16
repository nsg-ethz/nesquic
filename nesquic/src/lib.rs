use anyhow::Result;
use clap::ValueEnum;
use metrics::THROUGHPUT;
use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
use utils::{
    bin::{Client, ClientArgs, Server, ServerArgs},
    perf::{Request, Stats},
};

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub mod metrics;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Library {
    Quinn,
    Quiche,
    Msquic,
    Ngtcp,
}

impl Library {
    pub fn name(&self) -> String {
        format!("{:?}", self).to_lowercase()
    }

    pub fn version(&self) -> String {
        built_info::INDIRECT_DEPENDENCIES
            .iter()
            .find(|(name, _)| name == &self.name())
            .expect(format!("Failed to find version of {}", self.name()).as_str())
            .1
            .to_string()
    }
}

pub async fn run_client(lib: Library, args: ClientArgs) -> Result<()> {
    async fn run<C: Client>(args: ClientArgs) -> Result<Stats> {
        let req = Request::try_from(args.blob.clone())?;
        let mut client = C::new(args)?;
        client.connect().await?;

        let mut stats = Stats::new();

        stats.start_measurement();

        client.run().await?;
        stats.add_bytes(req.size)?;

        stats.stop_measurement()?;
        Ok(stats)
    }

    let stats = match lib {
        Library::Quinn => run::<QuinnClient>(args).await?,
        Library::Quiche => run::<QuicheClient>(args).await?,
        Library::Msquic => run::<MsQuicClient>(args).await?,
        Library::Ngtcp => unimplemented!("ngtcp"),
    };

    THROUGHPUT.observe(stats.throughputs().mean());
    Ok(())
}

pub async fn run_server(lib: Library, args: ServerArgs) -> Result<()> {
    match lib {
        Library::Quinn => {
            let mut server = QuinnServer::new(args)?;
            server.listen().await
        }
        Library::Quiche => {
            let mut server = QuicheServer::new(args)?;
            server.listen().await
        }
        Library::Msquic => {
            let mut server = MsQuicServer::new(args)?;
            server.listen().await
        }
        Library::Ngtcp => unimplemented!("ngtcp"),
    }
}
