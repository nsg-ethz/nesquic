use crate::metrics::{MetricsCollector, THROUGHPUT};
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand, ValueEnum};
use core_affinity::{self, CoreId};
use futures::future::Either::*;
use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
use std::{collections::HashMap, env, future::Future, mem::MaybeUninit};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::oneshot,
};
use tracing::{error, info, trace, warn};
use utils::bin::{Client, ClientArgs, Server, ServerArgs};

mod metrics;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Run as client
    Client(ClientLibArgs),
    /// Run as server
    Server(ServerLibArgs),
}

impl Command {
    fn args(&self) -> &CommonArgs {
        match self {
            Command::Client(args) => &args.common,
            Command::Server(args) => &args.common,
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct CommonArgs {
    #[clap(short, long, value_enum)]
    pub lib: Library,

    #[clap(short, long, value_enum)]
    pub job: Option<String>,

    #[clap(short = 'L')]
    pub labels: Option<Vec<String>>,

    #[clap(long)]
    pub quic_cpu: Option<usize>,

    #[clap(long)]
    pub metric_cpu: Option<usize>,
}

#[derive(Parser, Debug, Clone)]
pub struct ClientLibArgs {
    #[clap(flatten)]
    pub common: CommonArgs,

    #[clap(flatten)]
    pub client: ClientArgs,
}

#[derive(Parser, Debug, Clone)]
pub struct ServerLibArgs {
    #[clap(flatten)]
    pub common: CommonArgs,

    #[clap(flatten)]
    pub server: ServerArgs,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Library {
    Quinn,
    Quiche,
    Msquic,
    Ngtcp,
}

impl Library {
    fn name(&self) -> String {
        format!("{:?}", self).to_lowercase()
    }

    fn version(&self) -> String {
        built_info::INDIRECT_DEPENDENCIES
            .iter()
            .find(|(name, _)| name == &self.name())
            .expect(format!("Failed to find version of {}", self.name()).as_str())
            .1
            .to_string()
    }
}

async fn run_client(lib: Library, args: ClientArgs) -> Result<()> {
    let stats = match lib {
        Library::Quinn => {
            let mut client = QuinnClient::new(args)?;
            client.run().await?;
            client.stats().clone()
        }
        Library::Quiche => {
            let mut client = QuicheClient::new(args)?;
            client.run().await?;
            client.stats().clone()
        }
        Library::Msquic => {
            let mut client = MsQuicClient::new(args)?;
            client.run().await?;
            client.stats().clone()
        }
        Library::Ngtcp => unimplemented!("ngtcp"),
    };

    THROUGHPUT.observe(stats.throughputs().mean());
    Ok(())
}

async fn run_server(lib: Library, args: ServerArgs) -> Result<()> {
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

fn labels(cli: &Cli) -> HashMap<String, String> {
    let log_level = tracing::level_filters::LevelFilter::current().to_string();
    let mode = match cli.command {
        Command::Client(_) => String::from("client"),
        Command::Server(_) => String::from("server"),
    };

    let run_labels = cli
        .command
        .args()
        .labels
        .clone()
        .unwrap_or(vec![])
        .iter()
        .map(|h| {
            let hs: Vec<&str> = h.split_terminator(":").map(|s| s.trim()).collect();
            (hs[0].to_string(), hs[1].to_string())
        })
        .collect::<HashMap<String, String>>();

    let library = cli.command.args().lib;

    let mut labels = HashMap::new();
    labels.insert(String::from("log_level"), log_level);
    labels.insert(String::from("library"), library.name());
    labels.insert(String::from("mode"), mode);
    labels.insert(String::from("version"), library.version());
    labels.extend(run_labels.into_iter());

    labels
}

fn get_core_id(idx: usize) -> Result<CoreId> {
    let cores = core_affinity::get_core_ids();
    let Some(cores) = cores else {
        return Err(anyhow!("Core {idx} not available"));
    };
    if idx >= cores.len() {
        return Err(anyhow!("Core {idx} not available"));
    }
    Ok(cores[idx])
}

async fn select_with_term_signals<T>(future: impl Future<Output = T>) -> Option<T> {
    let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => None,
        _ = sigterm.recv() => None,
        res = future => Some(res),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    let cli = Cli::parse();
    let labels = labels(&cli);
    let job = cli.command.args().job.clone();

    // we use this to make sure the metrics are installed before starting the job
    let (job_start_tx, job_start_rx) = oneshot::channel();

    // this is used to push the metrics once the job is done
    let (job_done_tx, job_done_rx) = oneshot::channel();

    let quic_core = cli
        .command
        .args()
        .quic_cpu
        .map(|idx| get_core_id(idx))
        .transpose()?;
    let metric_core = cli
        .command
        .args()
        .metric_cpu
        .map(|idx| get_core_id(idx))
        .transpose()?;

    let monitor_handle = tokio::spawn(async move {
        if let Some(core) = metric_core {
            trace!("Set metric core to {}", core.id);
            core_affinity::set_for_current(core);
        }

        let mut open_obj = MaybeUninit::uninit();
        let mut monitor = MetricsCollector::new(&mut open_obj).expect("metrics collector");
        monitor.monitor_io().expect("monitor IO");

        job_start_tx.send(()).expect("Failed to start job");
        job_done_rx.await.expect("Failed to wait for job");

        if let Some(job) = job {
            if let Ok(push_gateway) = env::var("PR_PUSH_GATEWAY") {
                info!("Pushing metrics to {}", push_gateway);

                if let Some(Err(e)) =
                    select_with_term_signals(monitor.push_all(push_gateway.clone(), job, labels))
                        .await
                {
                    error!("Error pushing metrics to {}: {}", push_gateway, e);
                }
            }
        } else {
            if let Err(e) = monitor.report() {
                error!("Error reporting metrics: {}", e);
            }
        }

        drop(monitor);
    });

    if let Some(core) = quic_core {
        trace!("Set metric core to {}", core.id);
        core_affinity::set_for_current(core);
    }

    job_start_rx.await.expect("Failed to start job");

    let job = match &cli.command {
        Command::Client(args) => Left(run_client(args.common.lib, args.client.clone())),
        Command::Server(args) => Right(run_server(args.common.lib, args.server.clone())),
    };

    if let Some(Ok(())) = select_with_term_signals(job).await {
        trace!("Job completed");
    }

    if job_done_tx.send(()).is_ok() {
        monitor_handle.await?;
    } else {
        warn!("Pushing metrics potentially failed");
    }

    Ok(())
}
