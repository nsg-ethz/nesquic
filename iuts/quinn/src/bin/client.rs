use anyhow::Result;
use clap::Parser;
use quinn_iut::client::Client;
use utils::bin::{Client as ClientBin, ClientArgs};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let mut client = Client::new(ClientArgs::parse())?;
    client.run().await?;

    println!("{}", client.stats().summary());

    Ok(())
}
