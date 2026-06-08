use client::Client;
use common::run;
use server::Server;

mod client;
mod server;

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
