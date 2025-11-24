use anyhow::Result;
use lazy_static::lazy_static;
use libbpf_rs::{
    set_print,
    skel::{OpenSkel, SkelBuilder},
    Link, PrintLevel,
};
use prometheus::{opts, register_int_counter, IntCounter};
use prometheus_push::prometheus_crate::PrometheusMetricsPusher;
use reqwest::{Client, Url};
use std::{collections::HashMap, mem::MaybeUninit};
use tracing::{debug, info, warn};

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/metrics/metrics.skel.rs"
));

lazy_static! {
    pub static ref RUNS: IntCounter =
        register_int_counter!(opts!("nesquic_runs", "Nesquic benchmarking runs"))
            .expect("nesquic_runs");
    pub static ref IO_WRITEV: IntCounter =
        register_int_counter!(opts!("nesquic_io_writev", "writev calls"))
            .expect("nesquic_io_writev");
}

fn print(level: PrintLevel, msg: String) {
    let msg = msg.trim_start_matches("libbpf:").trim();

    match level {
        PrintLevel::Debug => debug!(target: "libbpf", "{}", msg),
        PrintLevel::Info => info!(target: "libbpf", "{}", msg),
        PrintLevel::Warn => warn!(target: "libbpf", "{}", msg),
    }
}

pub struct MetricsCollector<'obj> {
    skel: MetricsSkel<'obj>,
}

impl<'obj> MetricsCollector<'obj> {
    pub fn new(open_obj: &'obj mut MaybeUninit<libbpf_rs::OpenObject>) -> Result<Self> {
        set_print(Some((PrintLevel::Debug, print)));

        let skel_builder = MetricsSkelBuilder::default();
        let mut open_skel = skel_builder.open(open_obj)?;

        if tracing::enabled!(tracing::Level::DEBUG) {
            open_skel.progs.do_writev.set_log_level(1);
        }
        let skel = open_skel.load()?;

        Ok(Self { skel })
    }

    pub fn monitor_io(&self) -> Result<Link> {
        info!("Monitoring IO");
        let link = self.skel.progs.do_writev.attach()?;

        Ok(link)
    }

    fn update_metrics(&self) -> Result<()> {
        RUNS.inc();

        let num_calls = self
            .skel
            .maps
            .bss_data
            .as_ref()
            .unwrap()
            .do_writev_num_calls;
        IO_WRITEV.inc_by(num_calls as u64);

        Ok(())
    }

    pub async fn push_all<S: AsRef<str>>(
        &self,
        gateway: S,
        labels: HashMap<String, String>,
    ) -> Result<()> {
        self.update_metrics()?;

        let push_gateway: Url = Url::parse(gateway.as_ref())?;
        let client = Client::new();
        let metrics_pusher = PrometheusMetricsPusher::from(client, &push_gateway)?;

        let labels = labels
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<HashMap<&str, &str>>();

        metrics_pusher
            .push_all("nesquic", &labels, prometheus::gather())
            .await?;

        Ok(())
    }
}
