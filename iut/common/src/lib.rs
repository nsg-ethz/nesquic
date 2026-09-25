use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use core_affinity::{self, CoreId};
use futures::future::Either::*;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::{env, future::Future};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{info, trace};
use utils::{
    bin::{Client, ClientArgs, Server, ServerArgs},
    perf::{Request, Stats},
};

pub mod test;

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
    #[clap(short, long)]
    pub job: Option<String>,

    #[clap(short = 'L')]
    pub labels: Option<Vec<String>>,

    #[clap(long)]
    pub quic_cpu: Option<usize>,
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

async fn run_client<C: Client>(args: ClientArgs) -> Result<()> {
    let req = Request::try_from(args.blob.clone())?;
    let mut client = C::new(args)?;
    client.connect().await?;

    let mut stats = Stats::new();
    stats.start_measurement();
    client.run().await?;
    stats.add_bytes(req.len())?;
    stats.stop_measurement()?;

    println!("throughput: {}", stats.throughputs().mean());

    Ok(())
}

async fn run_server<S: Server>(args: ServerArgs) -> Result<()> {
    let mut s = S::new(args)?;
    s.listen().await
}

fn get_core_id(idx: usize) -> Result<CoreId> {
    let cores = core_affinity::get_core_ids();
    let Some(cores) = cores else {
        return Err(anyhow::anyhow!("Core {idx} not available"));
    };
    if idx >= cores.len() {
        return Err(anyhow::anyhow!("Core {idx} not available"));
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

/// Run the IUT binary: parse CLI and execute client or server.
///
/// Metrics are collected and uploaded by the preloaded `libnesquic.so`, which
/// reads `lib_name` and `lib_version` from `NQ_LIBRARY`/`NQ_LIBRARY_VERSION`
/// to tag every measurement.
pub async fn run<C: Client, S: Server>(lib_name: &str, lib_version: &str) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    // Read by libnesquic.so when it reports at exit.
    env::set_var("NQ_LIBRARY", lib_name);
    env::set_var("NQ_LIBRARY_VERSION", lib_version);

    let cli = Cli::parse();

    let quic_core = cli.command.args().quic_cpu.map(get_core_id).transpose()?;

    if let Some(core) = quic_core {
        trace!("Set quic core to {}", core.id);
        core_affinity::set_for_current(core);
    }

    let job = match &cli.command {
        Command::Client(args) => Left(run_client::<C>(args.client.clone())),
        Command::Server(args) => Right(run_server::<S>(args.server.clone())),
    };

    match select_with_term_signals(job).await {
        Some(Ok(())) => info!("Job completed"),
        Some(Err(e)) => bail!(e),
        _ => trace!("Job cancelled"),
    }

    Ok(())
}

pub fn bind_socket(addr: SocketAddr) -> Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))
        .context("create socket")?;

    if addr.is_ipv6() {
        socket.set_only_v6(false).context("set_only_v6")?;
    }

    socket
        .bind(&socket2::SockAddr::from(addr))
        .context("binding endpoint")?;

    Ok(socket.into())
}
