use anyhow::{anyhow, bail, Result};
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
    pub static ref IO_WRITEV_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_writev_num_calls", "writev calls"))
            .expect("io_writev_num_calls");
    pub static ref IO_WRITEV_DATA_SIZE: IntCounter =
        register_int_counter!(opts!("io_writev_data_size", "amount of data written"))
            .expect("io_writev_data_size");
    pub static ref IO_READV_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_readv_num_calls", "readv calls"))
            .expect("io_readv_num_calls");
    pub static ref IO_READV_DATA_SIZE: IntCounter =
        register_int_counter!(opts!("io_readv_data_size", "amount of data written"))
            .expect("io_readv_data_size");
    pub static ref IO_WRITE_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_write_num_calls", "write calls"))
            .expect("io_writev_num_calls");
    pub static ref IO_WRITE_DATA_SIZE: IntCounter =
        register_int_counter!(opts!("io_write_data_size", "amount of data written"))
            .expect("io_write_data_size");
    pub static ref IO_READ_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_read_num_calls", "read calls")).expect("io_read_num_calls");
    pub static ref IO_READ_DATA_SIZE: IntCounter =
        register_int_counter!(opts!("io_read_data_size", "amount of data written"))
            .expect("io_read_data_size");
    pub static ref IO_RECVFROM_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_recvfrom_num_calls", "recvfrom calls"))
            .expect("io_recvfrom_num_calls");
    pub static ref IO_RECVFROM_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_recvfrom_data_size",
        "amount of data received via recvfrom"
    ))
    .expect("io_recvfrom_data_size");
    pub static ref IO_SENDTO_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_sendto_num_calls", "sendto calls"))
            .expect("io_sendto_num_calls");
    pub static ref IO_SENDTO_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_sendto_data_size",
        "amount of data sent via sendto"
    ))
    .expect("io_sendto_data_size");
    pub static ref IO_SENDMSG_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_sendmsg_num_calls", "sendmsg calls"))
            .expect("io_sendmsg_num_calls");
    pub static ref IO_SENDMSG_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_sendmsg_data_size",
        "amount of data sent via sendmsg"
    ))
    .expect("io_sendmsg_data_size");
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

        let Some(ro_data) = open_skel.maps.rodata_data.as_mut() else {
            bail!("Failed to load rodata");
        };
        ro_data.MONITORED_PID = std::process::id();

        if tracing::enabled!(tracing::Level::DEBUG) {
            open_skel.progs.do_writev.set_log_level(1);
        }
        let skel = open_skel.load()?;

        Ok(Self { skel })
    }

    pub fn monitor_io(&self) -> Result<Vec<Link>> {
        info!("Monitoring IO");
        let ksys_read = self.skel.progs.ksys_read.attach()?;
        let ksys_write = self.skel.progs.ksys_write.attach()?;
        let do_writev = self.skel.progs.do_writev.attach()?;
        let do_readv = self.skel.progs.do_readv.attach()?;

        let sys_recvfrom = self.skel.progs.__sys_recvfrom.attach()?;
        let sys_recvmmsg = self.skel.progs.__sys_recvmmsg.attach()?;
        let sys_sendto = self.skel.progs.__sys_sendto.attach()?;
        let sys_sendmsg = self.skel.progs.__sys_sendmsg.attach()?;
        let sys_sendmmsg = self.skel.progs.__sys_sendmmsg.attach()?;

        Ok(vec![
            ksys_read,
            ksys_write,
            do_writev,
            do_readv,
            sys_recvfrom,
            sys_recvmmsg,
            sys_sendto,
            sys_sendmsg,
            sys_sendmmsg,
        ])
    }

    fn update_metrics(&self) -> Result<()> {
        RUNS.inc();

        let bss = self
            .skel
            .maps
            .bss_data
            .as_ref()
            .ok_or(anyhow!("Failed to load bss data"))?;

        IO_WRITEV_NUM_CALLS.inc_by(bss.do_writev_num_calls.into());
        IO_WRITEV_DATA_SIZE.inc_by(bss.do_writev_data_size.into());
        IO_READV_NUM_CALLS.inc_by(bss.do_readv_num_calls.into());
        IO_READV_DATA_SIZE.inc_by(bss.do_readv_data_size.into());

        IO_WRITE_NUM_CALLS.inc_by(bss.ksys_write_num_calls.into());
        IO_WRITE_DATA_SIZE.inc_by(bss.ksys_write_data_size.into());
        IO_READ_NUM_CALLS.inc_by(bss.ksys_read_num_calls.into());
        IO_READ_DATA_SIZE.inc_by(bss.ksys_read_data_size.into());

        IO_RECVFROM_NUM_CALLS.inc_by(bss.__sys_recvfrom_num_calls.into());
        IO_RECVFROM_DATA_SIZE.inc_by(bss.__sys_recvfrom_data_size.into());

        IO_SENDTO_NUM_CALLS.inc_by(bss.__sys_sendto_num_calls.into());
        IO_SENDTO_DATA_SIZE.inc_by(bss.__sys_sendto_data_size.into());

        IO_SENDMSG_NUM_CALLS.inc_by(bss.__sys_sendmsg_num_calls.into());
        IO_SENDMSG_DATA_SIZE.inc_by(bss.__sys_sendmsg_data_size.into());

        Ok(())
    }

    pub async fn push_all<S: AsRef<str>>(
        &self,
        gateway: S,
        job: S,
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
            .push_all(job.as_ref(), &labels, prometheus::gather())
            .await?;

        Ok(())
    }

    pub fn report(&self) -> Result<()> {
        self.update_metrics()?;
        for metric in prometheus::gather() {
            println!("{:?}", metric);
        }

        Ok(())
    }
}
