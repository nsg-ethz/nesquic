use anyhow::{bail, Result};
use lazy_static::lazy_static;
use libbpf_rs::{
    set_print,
    skel::{OpenSkel, SkelBuilder},
    Link, PrintLevel,
};
use prometheus::{
    histogram_opts, opts, register_histogram, register_int_counter, Histogram, IntCounter,
};
use prometheus_push::prometheus_crate::PrometheusMetricsPusher;
use reqwest::{Client, Url};
use std::{collections::HashMap, mem::MaybeUninit, time::Duration};
use tracing::{debug, error, info, trace, warn};

include!(concat!(env!("OUT_DIR"), "/metrics.skel.rs"));

unsafe impl plain::Plain for types::event_io {}

lazy_static! {
    pub static ref RUNS: IntCounter =
        register_int_counter!(opts!("nesquic_runs", "Nesquic benchmarking runs"))
            .expect("nesquic_runs");
    pub static ref IO_EVENTS: Vec<(Histogram, Histogram)> = {
        let calls = vec![
            "write", "writev", "send", "sendto", "sendmsg", "sendmmsg", "read", "readv", "recv",
            "recvfrom", "recvmsg", "recvmmsg",
        ];

        let evs: Vec<(Histogram, Histogram)> = calls
            .iter()
            .map(|name| {
                let hist = register_histogram!(histogram_opts!(
                    format!("io_{}_num_calls", name),
                    format!("{} calls", name)
                ))
                .expect(&format!("io_{}_num_calls", name));
                let hist2 = register_histogram!(histogram_opts!(
                    format!("io_{}_data_size", name),
                    format!("data volume in {}", name)
                ))
                .expect(&format!("io_{}_data_size", name));
                (hist, hist2)
            })
            .collect();

        evs
    };
}

fn print(level: PrintLevel, msg: String) {
    let msg = msg.trim_start_matches("libbpf:").trim();

    match level {
        PrintLevel::Debug => debug!(target: "libbpf", "{}", msg),
        PrintLevel::Info => info!(target: "libbpf", "{}", msg),
        PrintLevel::Warn => warn!(target: "libbpf", "{}", msg),
    }
}

fn process(ev: &[u8]) -> i32 {
    let Ok(ev) = plain::from_bytes::<types::event_io>(ev) else {
        return -1;
    };

    trace!("Processing event: {:?}", ev);

    let (ref num_calls, ref data_size) = IO_EVENTS[ev.syscall as usize];
    num_calls.observe(1.0);
    data_size.observe(ev.len.into());

    return 0;
}

pub struct MetricsCollector<'obj> {
    skel: MetricsSkel<'obj>,
    links: Option<Vec<Link>>,
}

impl<'obj> MetricsCollector<'obj> {
    pub fn new(open_obj: &'obj mut MaybeUninit<libbpf_rs::OpenObject>) -> Result<Self> {
        set_print(Some((PrintLevel::Debug, print)));

        let skel_builder = MetricsSkelBuilder::default();
        let mut open_skel = skel_builder.open(open_obj)?;

        let Some(rodata) = open_skel.maps.rodata_data.as_mut() else {
            bail!("Failed to load rodata");
        };
        rodata.MONITORED_PID = std::process::id();

        if tracing::enabled!(tracing::Level::DEBUG) {
            open_skel.progs.do_writev.set_log_level(1);
        }
        let skel = open_skel.load()?;

        Ok(Self { skel, links: None })
    }

    pub fn monitor_io(&mut self) -> Result<()> {
        info!("Monitoring IO");
        let ksys_read = self.skel.progs.ksys_read.attach()?;
        let ksys_write = self.skel.progs.ksys_write.attach()?;
        let do_writev = self.skel.progs.do_writev.attach()?;
        let do_readv = self.skel.progs.do_readv.attach()?;

        let sys_recvfrom = self.skel.progs.__sys_recvfrom.attach()?;
        let sys_recvmsg = self.skel.progs.__sys_recvmsg.attach()?;
        let sys_recvmmsg = self.skel.progs.__sys_recvmmsg.attach()?;
        let sys_sendto = self.skel.progs.__sys_sendto.attach()?;
        let sys_sendmsg = self.skel.progs.__sys_sendmsg.attach()?;
        let sys_sendmmsg = self.skel.progs.__sys_sendmmsg.attach()?;

        self.links = Some(vec![
            ksys_read,
            ksys_write,
            do_writev,
            do_readv,
            sys_recvfrom,
            sys_recvmsg,
            sys_recvmmsg,
            sys_sendto,
            sys_sendmsg,
            sys_sendmmsg,
        ]);

        let mut builder = libbpf_rs::RingBufferBuilder::new();
        builder.add(&self.skel.maps.events, |ev| process(ev))?;
        let ringbuf = builder.build()?;

        tokio::spawn(async move {
            trace!("Polling...");
            loop {
                if let Err(e) = ringbuf.poll(Duration::from_millis(100)) {
                    error!("Failed to poll ring buffer: {}", e);
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        Ok(())
    }

    pub async fn push_all<S: AsRef<str>>(
        &mut self,
        gateway: S,
        job: S,
        labels: HashMap<String, String>,
    ) -> Result<()> {
        // this flushes the ring buffer
        self.links.take();
        RUNS.inc();

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

    pub fn report(&mut self) -> Result<()> {
        // this flushes the ring buffer
        self.links.take();
        RUNS.inc();

        for metric in prometheus::gather() {
            println!("{:?}", metric);
        }

        Ok(())
    }
}
