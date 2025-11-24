use anyhow::Result;
use lazy_static::lazy_static;
use prometheus::{opts, register_int_gauge, IntGauge};
use prometheus_push::prometheus_crate::PrometheusMetricsPusher;
use reqwest::{Client, Url};
use std::collections::HashMap;

lazy_static! {
    // pub static ref HTTP_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
    //     opts!("http_requests_total", "HTTP requests total"),
    //     &["method", "path"]
    // )
    // .expect("Can't create a metric");
    pub static ref NESQUIC_RUNS: IntGauge =
        register_int_gauge!(opts!("nesquic_runs", "Nesquic benchmarking runs"))
            .expect("nesquic_runs");
    // pub static ref HTTP_RESPONSE_TIME_SECONDS: HistogramVec = register_histogram_vec!(
    //     "http_response_time_seconds",
    //     "HTTP response times",
    //     &["method", "path"],
    //     HTTP_RESPONSE_TIME_CUSTOM_BUCKETS.to_vec()
    // )
    // .expect("Can't create a metric");
}

pub async fn push_all<S: AsRef<str>>(gateway: S, labels: HashMap<String, String>) -> Result<()> {
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
