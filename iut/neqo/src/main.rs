mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let version = built_info::INDIRECT_DEPENDENCIES
        .iter()
        .find(|(name, _)| name.contains("neqo"))
        .map(|(_, v)| *v)
        .unwrap_or("unknown");

    iut_common::run::<neqo_iut::Client, neqo_iut::Server>("neqo", version).await
}
