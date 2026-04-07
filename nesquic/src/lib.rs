use anyhow::{bail, Result};
use clap::ValueEnum;
use metrics::THROUGHPUT;

/*

# Features

This project supports multiple QUIC implementations, which are gated behind Cargo features.
Since some of these implementations depend on C libraries with conflicting symbols,
we enforce that only one implementation can be enabled at a time.
Since there is not clear "default" implementation, we do not enable any by default,
and require users to explicitly select one.
Even if multiple implementations would not be sctructurally incompatible,
we still enforce this to avoid confusion and to ensure that the build process is deterministic.

Thus, the following features are available:
- `msquic` (not available at the moment)
- `quinn`
- `quiche`
- `neqo`
*/

// #[cfg(any(
//     all(feature = "msquic", feature = "quinn"),
//     all(feature = "msquic", feature = "quiche"),
//     all(feature = "msquic", feature = "neqo"),
//     all(feature = "quinn", feature = "quiche"),
//     all(feature = "quinn", feature = "neqo"),
//     all(feature = "quiche", feature = "neqo"),
//     ))]
#[cfg(any(
    all(feature = "quinn", feature = "quiche"),
    all(feature = "quinn", feature = "neqo"),
    all(feature = "quiche", feature = "neqo"),
    ))]
compile_error!(
    "Cannot enable two or more IUTs: must choose one per compilation."
);

// #[cfg(not(any(feature = "msquic", feature = "quinn", feature = "quiche", feature = "neqo")))]
#[cfg(not(any(feature = "quinn", feature = "quiche", feature = "neqo")))]
compile_error!(
    "Must enable one of the following features: msquic, quinn, quiche, neqo"
);

// #[cfg(feature = "msquic")]
// use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
#[cfg(feature = "quinn")]
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
#[cfg(feature = "quiche")]
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
#[cfg(feature = "neqo")]
use neqo_iut::{Client as NeqoClient, Server as NeqoServer};
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
    Neqo
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
        stats.add_bytes(req.size)?;

        stats.stop_measurement()?;
        Ok(stats)
    }

    let stats = match lib {
        #[cfg(feature = "quinn")]
        Library::Quinn => run::<QuinnClient>(args).await?,
        #[cfg(feature = "quiche")]
        Library::Quiche => run::<QuicheClient>(args).await?,
        #[cfg(feature = "neqo")]
        Library::Neqo => run::<NeqoClient>(args).await?,
        // #[cfg(feature = "msquic")]
        // Library::Msquic => run::<MsQuicClient>(args).await?,
        _ => bail!("selected library is not enabled in this build"),
    };

    THROUGHPUT.observe(stats.throughputs().mean());
    Ok(())
}

pub async fn run_server(lib: Library, args: ServerArgs) -> Result<()> {
    match lib {
        #[cfg(feature = "quinn")]
        Library::Quinn => { let mut s = QuinnServer::new(args)?; s.listen().await }
        #[cfg(feature = "quiche")]
        Library::Quiche => { let mut s = QuicheServer::new(args)?; s.listen().await }
        #[cfg(feature = "neqo")]
        Library::Neqo => { let mut s = NeqoServer::new(args)?; s.listen().await }
        // #[cfg(feature = "msquic")]
        // Library::Msquic => { let mut s = MsQuicServer::new(args)?; s.listen().await }
        _ => bail!("selected library is not enabled in this build"),
    }
}
