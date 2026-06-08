use anyhow::{anyhow, Result};
use clap::Parser;
use core_affinity::{self, CoreId};
use metrics::MetricsCollector;
use std::{collections::HashMap, env, mem::MaybeUninit};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, trace};

mod metrics;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about)]
pub struct Cli {
    #[clap(short, long)]
    pub job: Option<String>,

    #[clap(short = 'L')]
    pub labels: Option<Vec<String>>,

    #[clap(long)]
    pub cpu: Option<usize>,
}

fn labels(cli: &Cli) -> HashMap<String, String> {
    let log_level = tracing::level_filters::LevelFilter::current().to_string();
    let run_labels = cli
        .labels
        .clone()
        .unwrap_or(vec![])
        .iter()
        .map(|h| {
            let hs: Vec<&str> = h.split_terminator(":").map(|s| s.trim()).collect();
            (hs[0].to_string(), hs[1].to_string())
        })
        .collect::<HashMap<String, String>>();
    trace!("Run labels: {:?}", run_labels);

    let mut labels = HashMap::new();
    labels.insert(String::from("log_level"), log_level);
    labels.insert(String::from("version"), built_info::PKG_VERSION.into());
    labels.extend(run_labels.into_iter());

    trace!("Final labels: {:?}", labels);

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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    let cli = Cli::parse();
    let labels = labels(&cli);
    let job = cli.job.clone();
    let core = cli.cpu.map(|idx| get_core_id(idx)).transpose()?;

    if let Some(core) = core {
        trace!("Set metric core to {}", core.id);
        core_affinity::set_for_current(core);
    }

    let mut open_obj = MaybeUninit::uninit();
    let mut monitor = MetricsCollector::new(&mut open_obj).expect("metrics collector");
    monitor.monitor_io().expect("monitor IO");

    let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => (),
        _ = sigterm.recv() => (),
    }

    if let Some(job) = job {
        if let (Ok(url), Ok(token), Ok(org), Ok(bucket)) = (
            env::var("INFLUX_URL"),
            env::var("INFLUX_TOKEN"),
            env::var("INFLUX_ORG"),
            env::var("INFLUX_BUCKET"),
        ) {
            info!("Pushing metrics to InfluxDB at {}", url);

            if let Err(e) = monitor.push_all(url, token, org, bucket, job, labels).await {
                error!("Error pushing metrics to InfluxDB: {}", e);
            }
        }
    } else {
        if let Err(e) = monitor.report() {
            error!("Error reporting metrics: {}", e);
        }
    }

    Ok(())
}
