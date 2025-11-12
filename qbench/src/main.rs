use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use quiche_iut::{Client as QuicheClient, Server as QuicheServer};
use quinn_iut::{Client as QuinnClient, Server as QuinnServer};
use tokio::signal::unix::{signal, SignalKind};
use utils::bin::{Client, ClientArgs, Server, ServerArgs};

#[derive(Parser, Debug)]
#[clap(author, version, about)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run as client
    Client(ClientLibArgs),
    /// Run as server
    Server(ServerLibArgs),
}

#[derive(Parser, Debug)]
pub struct ClientLibArgs {
    #[clap(short, long, value_enum)]
    pub lib: Library,

    #[clap(flatten)]
    pub client_args: ClientArgs,
}

#[derive(Parser, Debug)]
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
        Library::Msquic => unimplemented!("msquic"),
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
        Library::Msquic => unimplemented!("msquic"),
        Library::Ngtcp => unimplemented!("ngtcp"),
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    tokio::spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => std::process::exit(0),
            _ = sigterm.recv() => std::process::exit(0),
        }
    });

    let cli = Cli::parse();
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
}
