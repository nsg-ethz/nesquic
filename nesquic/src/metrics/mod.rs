use anyhow::{bail, Result};
use lazy_static::lazy_static;
use libbpf_rs::{
    set_print,
    skel::{OpenSkel, SkelBuilder},
    Link, PrintLevel,
};
use reqwest::Client;
use std::{
    collections::HashMap,
    mem::MaybeUninit,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{debug, error, info, trace, warn};

include!(concat!(env!("OUT_DIR"), "/metrics.skel.rs"));

unsafe impl plain::Plain for types::event_io {}

lazy_static! {
    pub static ref SYSCALLS: Vec<&'static str> = vec![
        "write", "writev", "send", "sendto", "sendmsg", "sendmmsg", "read", "readv", "recv",
        "recvfrom", "recvmsg", "recvmmsg",
    ];
    /// Bytes transferred per syscall invocation, keyed by syscall name.
    pub static ref SYSCALL_VOLUMES: Mutex<HashMap<String, Vec<f64>>> =
        Mutex::new(HashMap::new());
    /// Throughput samples (Mbps) recorded by the client after each run.
    pub static ref THROUGHPUT_SAMPLES: Mutex<Vec<f64>> = Mutex::new(Vec::new());
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

    let syscall = SYSCALLS[ev.syscall as usize];
    let volume_kb = ev.len as f64 / 1000.0;

    SYSCALL_VOLUMES
        .lock()
        .unwrap()
        .entry(syscall.to_string())
        .or_default()
        .push(volume_kb);

    0
}

/// Drain global metric state into InfluxDB line-protocol strings.
/// All mutex locks are acquired and released inside this sync function,
/// so no MutexGuard crosses an await boundary in the async push_all.
fn collect_line_protocol(tag_str: &str, timestamp_ns: u128) -> Result<String> {
    let mut lines: Vec<String> = Vec::new();

    let throughput_samples = THROUGHPUT_SAMPLES.lock().unwrap();
    if !throughput_samples.is_empty() {
        let mean = throughput_samples.iter().sum::<f64>() / throughput_samples.len() as f64;
        lines.push(format!(
            "nesquic{} throughput={} {}",
            tag_str, mean, timestamp_ns
        ));
    }

    let mut volumes = SYSCALL_VOLUMES.lock().unwrap();
    for (syscall, data) in volumes.drain() {
        let count = data.len();
        let sum: f64 = data.iter().sum();

        let syscall_tag = format!("{},syscall={}", tag_str, &syscall);
        lines.push(format!(
            "nesquic_io{} volume_kb_sum={},count={}i {}",
            syscall_tag, sum, count, timestamp_ns
        ));
    }

    if lines.is_empty() {
        bail!("No metrics to push");
    }
    Ok(lines.join("\n"))
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
        let skel = open_skel.load()?;

        Ok(Self { skel, links: None })
    }

    pub fn monitor_io(&mut self) -> Result<()> {
        info!("Monitoring IO");
        let read = self.skel.progs.read.attach()?;
        let write = self.skel.progs.write.attach()?;
        let writev = self.skel.progs.writev.attach()?;
        let readv = self.skel.progs.readv.attach()?;

        let recvfrom = self.skel.progs.recvfrom.attach()?;
        let recvmsg = self.skel.progs.recvmsg.attach()?;
        let recvmmsg = self.skel.progs.recvmmsg.attach()?;
        let sendto = self.skel.progs.sendto.attach()?;
        let sendmsg = self.skel.progs.sendmsg.attach()?;
        let sendmmsg = self.skel.progs.sendmmsg.attach()?;

        self.links = Some(vec![
            read, write, writev, readv, recvfrom, recvmsg, recvmmsg, sendto, sendmsg, sendmmsg,
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

    /// Push all collected metrics to InfluxDB as a single point-in-time measurement.
    ///
    /// Writes two measurement types:
    /// - `nesquic`: throughput (one point per run)
    /// - `nesquic_io`: per-syscall stats (one point per syscall type per run)
    pub async fn push_all(
        &mut self,
        url: String,
        token: String,
        org: String,
        bucket: String,
        job: String,
        tags: HashMap<String, String>,
    ) -> Result<()> {
        // Dropping links flushes the ring buffer
        self.links.take();

        let timestamp_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();

        let mut all_tags = tags;
        all_tags.insert("job".to_string(), job);

        fn build_tag_str(tags: &HashMap<String, String>) -> String {
            fn escape_tag(s: &str) -> String {
                s.replace(',', "\\,")
                    .replace('=', "\\=")
                    .replace(' ', "\\ ")
            }

            let mut parts: Vec<String> = tags
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| format!("{}={}", escape_tag(k), escape_tag(v)))
                .collect();
            parts.sort();
            if parts.is_empty() {
                String::new()
            } else {
                format!(",{}", parts.join(","))
            }
        }

        let tag_str = build_tag_str(&all_tags);

        // Collect data into plain strings before any await (MutexGuard is not Send)
        let body = collect_line_protocol(&tag_str, timestamp_ns)?;
        let line_count = body.lines().count();

        let write_url = format!(
            "{}/api/v2/write?org={}&bucket={}&precision=ns",
            url, org, bucket
        );

        info!("Pushing {} line(s) to {}", line_count, write_url);
        for line in body.lines() {
            debug!("  > {}", line);
        }

        let client = Client::new();
        let resp = client
            .post(&write_url)
            .header("Authorization", format!("Token {}", token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {} (kind: {:?})", e, e.url()))?;

        let status = resp.status();
        info!("InfluxDB responded with HTTP {}", status);

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("InfluxDB write failed {}: {}", status, body);
        }

        info!("Metrics written to InfluxDB (bucket: {}/{})", org, bucket);
        Ok(())
    }

    pub fn report(&mut self) -> Result<()> {
        self.links.take();

        let throughput_samples = THROUGHPUT_SAMPLES.lock().unwrap();
        if !throughput_samples.is_empty() {
            let mean = throughput_samples.iter().sum::<f64>() / throughput_samples.len() as f64;
            println!("throughput: {:.3} Mbps", mean);
        }
        drop(throughput_samples);

        let volumes = SYSCALL_VOLUMES.lock().unwrap();
        for (syscall, data) in volumes.iter() {
            let count = data.len();
            let sum: f64 = data.iter().sum();
            println!("{}: count={}, volume_kb_sum={:.3}", syscall, count, sum);
        }

        Ok(())
    }
}
