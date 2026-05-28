mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let version = built_info::INDIRECT_DEPENDENCIES
        .iter()
        .find(|(name, _)| name.contains("quinn"))
        .map(|(_, v)| *v)
        .unwrap_or("unknown");

    iut_common::run::<quinn_iut::Client, quinn_iut::Server>("quinn", version).await
}
