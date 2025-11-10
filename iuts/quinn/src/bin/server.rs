use anyhow::Result;
use clap::Parser;
use quinn_iut::server::Server;
use utils::bin::{Server as ServerBin, ServerArgs};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let server = Server::new(ServerArgs::parse())?;
    server.listen().await?;

    Ok(())
}
