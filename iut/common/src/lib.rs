use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use core_affinity::{self, CoreId};
use futures::future::Either::*;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::{collections::HashMap, env, future::Future, mem::MaybeUninit};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::oneshot,
};
use tracing::{error, info, trace, warn};
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

async fn run_client<C: Client>(args: ClientArgs) -> Result<()> {
    let req = Request::try_from(args.blob.clone())?;
    let mut client = C::new(args)?;
    client.connect().await?;

    let mut stats = Stats::new();
    stats.start_measurement();
    client.run().await?;
    stats.add_bytes(req.len())?;
    stats.stop_measurement()?;

    // THROUGHPUT_SAMPLES
    //     .lock()
    //     .unwrap()
    //     .push(stats.throughputs().mean());

    Ok(())
}

async fn run_server<S: Server>(args: ServerArgs) -> Result<()> {
    let mut s = S::new(args)?;
    s.listen().await
}

fn build_labels(cli: &Cli, lib_name: &str, lib_version: &str) -> HashMap<String, String> {
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
        .unwrap_or_default()
        .iter()
        .map(|h| {
            let hs: Vec<&str> = h.split_terminator(':').map(|s| s.trim()).collect();
            (hs[0].to_string(), hs[1].to_string())
        })
        .collect::<HashMap<String, String>>();

    let mut labels = HashMap::new();
    labels.insert(String::from("log_level"), log_level);
    labels.insert(String::from("library"), lib_name.to_string());
    labels.insert(String::from("mode"), mode);
    labels.insert(String::from("version"), lib_version.to_string());
    labels.extend(run_labels);
    labels
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

/// Run the IUT binary: parse CLI, collect eBPF metrics, and execute client or server.
///
/// `lib_name` and `lib_version` are embedded as InfluxDB tags on every measurement.
pub async fn run<C: Client, S: Server>(lib_name: &str, lib_version: &str) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    let cli = Cli::parse();
    let labels = build_labels(&cli, lib_name, lib_version);
    let job = cli.command.args().job.clone();

    // let (job_start_tx, job_start_rx) = oneshot::channel();
    // let (job_done_tx, job_done_rx) = oneshot::channel();

    let quic_core = cli.command.args().quic_cpu.map(get_core_id).transpose()?;
    let metric_core = cli.command.args().metric_cpu.map(get_core_id).transpose()?;

    // let monitor_handle = tokio::spawn(async move {
    //     if let Some(core) = metric_core {
    //         trace!("Set metric core to {}", core.id);
    //         core_affinity::set_for_current(core);
    //     }

    //     let mut open_obj = MaybeUninit::uninit();
    //     let mut monitor = MetricsCollector::new(&mut open_obj).expect("metrics collector");
    //     monitor.monitor_io().expect("monitor IO");

    //     _ = job_start_tx.send(());
    //     _ = job_done_rx.await;

    //     if let Some(job) = job {
    //         if let (Ok(url), Ok(token), Ok(org), Ok(bucket)) = (
    //             env::var("INFLUX_URL"),
    //             env::var("INFLUX_TOKEN"),
    //             env::var("INFLUX_ORG"),
    //             env::var("INFLUX_BUCKET"),
    //         ) {
    //             info!("Pushing metrics to InfluxDB at {}", url);

    //             if let Some(Err(e)) =
    //                 select_with_term_signals(monitor.push_all(url, token, org, bucket, job, labels))
    //                     .await
    //             {
    //                 error!("Error pushing metrics to InfluxDB: {}", e);
    //             }
    //         }
    //     } else if let Err(e) = monitor.report() {
    //         error!("Error reporting metrics: {}", e);
    //     }
    // });

    if let Some(core) = quic_core {
        trace!("Set quic core to {}", core.id);
        core_affinity::set_for_current(core);
    }

    // job_start_rx.await.expect("Failed to start job");

    let job = match &cli.command {
        Command::Client(args) => Left(run_client::<C>(args.client.clone())),
        Command::Server(args) => Right(run_server::<S>(args.server.clone())),
    };

    match select_with_term_signals(job).await {
        Some(Ok(())) => info!("Job completed"),
        Some(Err(e)) => bail!(e),
        _ => trace!("Job cancelled"),
    }

    // if job_done_tx.send(()).is_ok() {
    //     monitor_handle.await?;
    // } else {
    //     warn!("Pushing metrics potentially failed");
    // }

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
