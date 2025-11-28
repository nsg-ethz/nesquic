use anyhow::{bail, Result};
use lazy_static::lazy_static;
use libbpf_rs::{
    set_print,
    skel::{OpenSkel, SkelBuilder},
    Link, PrintLevel,
};
use prometheus::{opts, register_int_counter, IntCounter};
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
    pub static ref IO_RECV_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_recv_num_calls", "recv calls")).expect("io_recv_num_calls");
    pub static ref IO_RECV_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_recv_data_size",
        "amount of data received via recv"
    ))
    .expect("io_recv_data_size");
    pub static ref IO_RECVMSG_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_recvmsg_num_calls", "recvmsg calls"))
            .expect("io_recvmsg_num_calls");
    pub static ref IO_RECVMSG_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_recvmsg_data_size",
        "amount of data received via recvmsg"
    ))
    .expect("io_recvmsg_data_size");
    pub static ref IO_RECVMMSG_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_recvmmsg_num_calls", "recvmmsg calls"))
            .expect("io_recvmmsg_num_calls");
    pub static ref IO_RECVMMSG_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_recvmmsg_data_size",
        "amount of data received via recvmmsg"
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
    pub static ref IO_SEND_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_send_num_calls", "send calls")).expect("io_send_num_calls");
    pub static ref IO_SEND_DATA_SIZE: IntCounter =
        register_int_counter!(opts!("io_send_data_size", "amount of data sent via send"))
            .expect("io_send_data_size");
    pub static ref IO_SENDMSG_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_sendmsg_num_calls", "sendmsg calls"))
            .expect("io_sendmsg_num_calls");
    pub static ref IO_SENDMSG_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_sendmsg_data_size",
        "amount of data sent via sendmsg"
    ))
    .expect("io_sendmsg_data_size");
    pub static ref IO_SENDMMSG_NUM_CALLS: IntCounter =
        register_int_counter!(opts!("io_sendmmsg_num_calls", "sendmmsg calls"))
            .expect("io_sendmmsg_num_calls");
    pub static ref IO_SENDMMSG_DATA_SIZE: IntCounter = register_int_counter!(opts!(
        "io_sendmmsg_data_size",
        "amount of data sent via sendmmsg"
    ))
    .expect("io_sendmmsg_data_size");
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

    if ev.syscall == 1 {
        IO_WRITE_NUM_CALLS.inc();
        IO_WRITE_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 2 {
        IO_READ_NUM_CALLS.inc();
        IO_READ_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 3 {
        IO_WRITEV_NUM_CALLS.inc();
        IO_WRITEV_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 4 {
        IO_READV_NUM_CALLS.inc();
        IO_READV_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 5 {
        IO_RECV_NUM_CALLS.inc();
        IO_RECV_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 6 {
        IO_RECVFROM_NUM_CALLS.inc();
        IO_RECVFROM_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 7 {
        IO_RECVMSG_NUM_CALLS.inc();
        IO_RECVMSG_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 8 {
        IO_RECVMMSG_NUM_CALLS.inc();
        IO_RECVMMSG_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 9 {
        IO_SEND_NUM_CALLS.inc();
        IO_SEND_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 10 {
        IO_SENDTO_NUM_CALLS.inc();
        IO_SENDTO_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 11 {
        IO_SENDMSG_NUM_CALLS.inc();
        IO_SENDMSG_DATA_SIZE.inc_by(ev.len as u64);
    } else if ev.syscall == 12 {
        IO_SENDMMSG_NUM_CALLS.inc();
        IO_SENDMMSG_DATA_SIZE.inc_by(ev.len as u64);
    }

    return 0;
}

pub struct MetricsCollector<'obj> {
    skel: MetricsSkel<'obj>,
    links: Vec<Link>,
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

        Ok(Self {
            skel,
            links: Vec::new(),
        })
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

        self.links = vec![
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
        ];

        let mut builder = libbpf_rs::RingBufferBuilder::new();
        builder.add(&self.skel.maps.events, |ev| process(ev))?;
        let ringbuf = builder.build()?;
        // let fd = AsyncFd::with_interest(ringbuf.epoll_fd(), Interest::READABLE)?;

        tokio::spawn(async move {
            trace!("Polling...");
            loop {
                // let guard = fd
                //     .readable()
                //     .await
                //     .expect("Failed to read from ring buffer");

                // drop(guard)

                // ringbuf
                //     .poll(Duration::from_secs(0))
                //     .expect("Failed to poll ring buffer");

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
        self.links = Vec::new();
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
        self.links = Vec::new();
        RUNS.inc();

        for metric in prometheus::gather() {
            println!("{:?}", metric);
        }

        Ok(())
    }
}
