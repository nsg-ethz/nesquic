use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
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
    pub lib: Lib,

    #[clap(flatten)]
    pub client_args: ClientArgs,
}

#[derive(Parser, Debug)]
pub struct ServerLibArgs {
    #[clap(short, long, value_enum)]
    pub lib: Lib,

    #[clap(flatten)]
    pub server_args: ServerArgs,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Lib {
    Quinn,
    Quiche,
    Msquic,
    Ngtcp,
}

async fn run_client(lib: Lib, args: ClientArgs) -> Result<()> {
    let mut client = match lib {
        Lib::Quinn => QuinnClient::new(args)?,
        Lib::Quiche => unimplemented!("quiche"),
        Lib::Msquic => unimplemented!("msquic"),
        Lib::Ngtcp => unimplemented!("ngtcp"),
    };

    client.run().await
}

async fn run_server(lib: Lib, args: ServerArgs) -> Result<()> {
    let mut server = match lib {
        Lib::Quinn => QuinnServer::new(args)?,
        Lib::Quiche => unimplemented!("quiche"),
        Lib::Msquic => unimplemented!("msquic"),
        Lib::Ngtcp => unimplemented!("ngtcp"),
    };

    server.listen().await
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
