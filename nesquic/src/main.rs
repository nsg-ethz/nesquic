use crate::metrics::*;
use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use msquic_iut::{Client as MsQuicClient, Server as MsQuicServer};
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
use std::{collections::HashMap, env};
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

#[derive(Parser, Debug, Clone)]
pub struct ClientLibArgs {
    #[clap(short, long, value_enum)]
    pub lib: Library,

    #[clap(flatten)]
    pub client_args: ClientArgs,
}

#[derive(Parser, Debug, Clone)]
pub struct ServerLibArgs {
    #[clap(short, long, value_enum)]
    pub lib: Library,

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

async fn push_metrics(cli: Cli) -> Result<()> {
    if let Ok(push_gateway) = env::var("PR_PUSH_GATEWAY") {
        info!("Pushing metrics to {}", push_gateway);

        let log_level = tracing::level_filters::LevelFilter::current().to_string();
        let mode = match cli.command {
            Command::Client(_) => "client",
            Command::Server(_) => "server",
        };
        let library = match cli.command {
            Command::Client(args) => args.lib.clone(),
            Command::Server(args) => args.lib.clone(),
        };
        let library = format!("{:?}", library);

        let mut labels = HashMap::new();
        labels.insert("log_level", log_level.as_str());
        labels.insert("library", library.as_str());
        labels.insert("mode", mode);

        metrics::push_all(push_gateway, &labels).await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    let cli = Cli::parse();
    let term_cli = cli.clone();
    tokio::spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                if let Err(e) = push_metrics(term_cli).await {
                    error!("Error pushing metrics: {}", e);
                }
                std::process::exit(0);
            },
            _ = sigterm.recv() => {
                if let Err(e) = push_metrics(term_cli).await {
                    error!("Error pushing metrics: {}", e);
                }
                std::process::exit(0);
            },
        }
    });

    NESQUIC_RUNS.inc();
    match &cli.command {
        Command::Client(args) => {
            run_client(args.lib, args.client_args.clone())
                .await
                .unwrap();
        }
        Command::Server(args) => {
            run_server(args.lib, args.server_args.clone())
                .await
                .unwrap();
        }
    }

    Ok(())
}
