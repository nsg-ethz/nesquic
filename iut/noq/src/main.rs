use client::Client;
use common::run;
use server::Server;
use std::sync::Arc;
use noq::TransportConfig;
use utils::perf::{CONNECTION_WINDOW, IDLE_TIMEOUT, STREAM_WINDOW};

mod client;
mod server;

fn transport_config() -> Arc<TransportConfig> {
    let mut config = TransportConfig::default();
    config
        .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().expect("idle timeout")))
        .stream_receive_window(STREAM_WINDOW.into())
        .receive_window(CONNECTION_WINDOW.into());
    Arc::new(config)
}

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let version = built_info::INDIRECT_DEPENDENCIES
        .iter()
        .find(|(name, _)| name.contains("noq"))
        .map(|(_, v)| *v)
        .unwrap_or("unknown");

    run::<Client, Server>("noq", version).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::test;

    #[tokio::test]
    async fn test_connectivity() {
        test::connectivity::<Client, Server>().await;
    }
}
