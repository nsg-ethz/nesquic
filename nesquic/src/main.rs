use crate::metrics::MetricsCollector;
use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
use std::{collections::HashMap, env, mem::MaybeUninit};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info};
use utils::bin::{Client, ClientArgs, Server, ServerArgs};

mod metrics;

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

async fn run_client(lib: Library, args: ClientArgs) -> Result<()> {
    match lib {
        Library::Quinn => {
            let mut client = QuinnClient::new(args)?;
            client.run().await
        }
        Library::Quiche => {
            let mut client = QuicheClient::new(args)?;
            client.run().await
        }
        Library::Msquic => {
            let mut client = MsQuicClient::new(args)?;
            client.run().await
        }
        Library::Ngtcp => unimplemented!("ngtcp"),
    }
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
    fn to_string<V: std::fmt::Debug>(v: &V) -> String {
        format!("{:?}", v).to_lowercase()
    }

    let log_level = tracing::level_filters::LevelFilter::current().to_string();
    let mode = match cli.command {
        Command::Client(_) => String::from("client"),
        Command::Server(_) => String::from("server"),
    };
    let library = to_string(&cli.command.lib());

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

    let mut labels = HashMap::new();
    labels.insert(String::from("log_level"), log_level);
    labels.insert(String::from("library"), library);
    labels.insert(String::from("mode"), mode);
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
    tokio::spawn(async move {
        let mut open_obj = MaybeUninit::uninit();
        let monitor = MetricsCollector::new(&mut open_obj).expect("metrics collector");
        let io_link = monitor.monitor_io();

        let terminate = async move {
            drop(io_link);

            if let Some(job) = job {
                if let Ok(push_gateway) = env::var("PR_PUSH_GATEWAY") {
                    info!("Pushing metrics to {}", push_gateway);
                    if let Err(e) = monitor.push_all(push_gateway, job, labels).await {
                        error!("Error pushing metrics: {}", e);
                    }
                }
            }

            std::process::exit(0);
        };

        let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => terminate.await,
            _ = sigterm.recv() => terminate.await,
        }
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

    Ok(())
}
