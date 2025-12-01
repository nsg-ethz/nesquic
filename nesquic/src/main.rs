use crate::metrics::{MetricsCollector, THROUGHPUT};
use anyhow::Result;
use chrono::prelude::*;
use clap::{Parser, Subcommand, ValueEnum};
use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
use std::{collections::HashMap, env, mem::MaybeUninit};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::oneshot,
};
use tracing::{error, info, warn};
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
    fn job(&self) -> Option<String> {
        match self {
            Command::Client(args) => args.job.clone(),
            Command::Server(args) => args.job.clone(),
        }
    }

    fn lib(&self) -> Library {
        match self {
            Command::Client(args) => args.lib.clone(),
            Command::Server(args) => args.lib.clone(),
        }
    }

    fn labels(&self) -> Option<Vec<String>> {
        match self {
            Command::Client(args) => args.labels.clone(),
            Command::Server(args) => args.labels.clone(),
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct ClientLibArgs {
    #[clap(short, long, value_enum)]
    pub lib: Library,

    #[clap(short, long, value_enum)]
    pub job: Option<String>,

    #[clap(short = 'L')]
    pub labels: Option<Vec<String>>,

    #[clap(flatten)]
    pub client_args: ClientArgs,
}

#[derive(Parser, Debug, Clone)]
pub struct ServerLibArgs {
    #[clap(short, long, value_enum)]
    pub lib: Library,

    #[clap(short, long, value_enum)]
    pub job: Option<String>,

    #[clap(short = 'L')]
    pub labels: Option<Vec<String>>,

    #[clap(flatten)]
    pub server_args: ServerArgs,
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
        .labels()
        .unwrap_or(vec![])
        .iter()
        .map(|h| {
            let hs: Vec<&str> = h.split_terminator(":").map(|s| s.trim()).collect();
            (hs[0].to_string(), hs[1].to_string())
        })
        .collect::<HashMap<String, String>>();

    let library = cli.command.lib();
    let date = Utc::now().to_string();

    let mut labels = HashMap::new();
    labels.insert(String::from("log_level"), log_level);
    labels.insert(String::from("library"), library.name());
    labels.insert(String::from("mode"), mode);
    labels.insert(String::from("version"), library.version());
    labels.insert(String::from("run_id"), date);
    labels.extend(run_labels.into_iter());

    labels
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    let cli = Cli::parse();
    let labels = labels(&cli);
    let job = cli.command.job();
    let (tx, rx) = oneshot::channel();

    let monitor_handle = tokio::spawn(async move {
        let mut open_obj = MaybeUninit::uninit();
        let mut monitor = MetricsCollector::new(&mut open_obj).expect("metrics collector");
        monitor.monitor_io().expect("monitor IO");

        let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
            _ = rx => {},
        }

        if let Some(job) = job {
            if let Ok(push_gateway) = env::var("PR_PUSH_GATEWAY") {
                info!("Pushing metrics to {}", push_gateway);
                if let Err(e) = monitor.push_all(push_gateway, job, labels).await {
                    error!("Error pushing metrics: {}", e);
                }
            }
        } else {
            if let Err(e) = monitor.report() {
                error!("Error reporting metrics: {}", e);
            }
        }

        drop(monitor);
        std::process::exit(0);
    });

    match &cli.command {
        Command::Client(args) => {
            run_client(args.lib, args.client_args.clone())
                .await
                .expect("run_client");
        }
        Command::Server(args) => {
            run_server(args.lib, args.server_args.clone())
                .await
                .expect("run_server");
        }
    }

    if tx.send(()).is_ok() {
        monitor_handle.await?;
    } else {
        warn!("Pushing metrics potentially failed");
    }

    Ok(())
}
