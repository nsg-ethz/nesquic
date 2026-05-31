use anyhow::{bail, Result};
use clap::ValueEnum;
use metrics::THROUGHPUT_SAMPLES;

// #[cfg(feature = "msquic")]
// use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
#[cfg(feature = "neqo")]
use neqo_iut::{Client as NeqoClient, Server as NeqoServer};
#[cfg(feature = "noq")]
use noq_iut::{Client as NoqClient, Server as NoqServer};
#[cfg(feature = "quiche")]
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
#[cfg(feature = "quinn")]
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
    Neqo,
    Noq,
}

impl Library {
    pub fn name(&self) -> String {
        format!("{:?}", self).to_lowercase()
    }

    pub fn version(&self) -> String {
        built_info::INDIRECT_DEPENDENCIES
            .iter()
            .find(|(name, _)| name.contains(&self.name()))
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
        stats.add_bytes(req.len())?;

        stats.stop_measurement()?;
        Ok(stats)
    }

    let stats: Stats = match lib {
        #[cfg(feature = "quinn")]
        Library::Quinn => run::<QuinnClient>(args).await?,
        #[cfg(feature = "quiche")]
        Library::Quiche => run::<QuicheClient>(args).await?,
        #[cfg(feature = "neqo")]
        Library::Neqo => run::<NeqoClient>(args).await?,
        #[cfg(feature = "noq")]
        Library::Noq => run::<NoqClient>(args).await?,
        // #[cfg(feature = "msquic")]
        // Library::Msquic => run::<MsQuicClient>(args).await?,
        _ => bail!("selected library is not enabled in this build"),
    };

    THROUGHPUT_SAMPLES
        .lock()
        .unwrap()
        .push(stats.throughputs().mean());
    Ok(())
}

pub async fn run_server(lib: Library, args: ServerArgs) -> Result<()> {
    match lib {
        #[cfg(feature = "quinn")]
        Library::Quinn => {
            let mut s = QuinnServer::new(args)?;
            s.listen().await
        }
        #[cfg(feature = "quiche")]
        Library::Quiche => {
            let mut s = QuicheServer::new(args)?;
            s.listen().await
        }
        #[cfg(feature = "neqo")]
        Library::Neqo => {
            let mut s = NeqoServer::new(args)?;
            s.listen().await
        }
        #[cfg(feature = "noq")]
        Library::Noq => {
            let mut s = NoqServer::new(args)?;
            s.listen().await
        }
        // #[cfg(feature = "msquic")]
        // Library::Msquic => { let mut s = MsQuicServer::new(args)?; s.listen().await }
        _ => bail!("selected library is not enabled in this build"),
    }
}
