use crate::perf::MAX_STREAMS;
use anyhow::Result;
use clap::Parser;
use std::{future::Future, net::SocketAddr, time::Duration};
use url::Url;

#[derive(Parser, Clone, Debug)]
#[clap(name = "client")]
pub struct ClientArgs {
    /// the address of the server
    #[clap(default_value = "https://127.0.0.1:4433")]
    pub url: Url,

    /// TLS certificate in PEM format
    #[clap(long)]
    pub cert: String,

    #[clap(short, long)]
    pub blob: String,

    /// number of QUIC connections, each on its own UDP socket
    #[clap(short, long, default_value = "1", value_parser = clap::value_parser!(u64).range(1..))]
    pub connections: u64,

    /// number of concurrent requests per connection
    #[clap(short, long, default_value = "1", value_parser = clap::value_parser!(u64).range(1..=MAX_STREAMS))]
    pub streams: u64,

    /// seconds during which every finished request is followed by another
    #[clap(short, long, value_parser = clap::value_parser!(u64).range(1..))]
    pub duration: Option<u64>,
}

impl ClientArgs {
    pub fn test() -> Self {
        ClientArgs {
            url: Url::parse("https://127.0.0.1:4433").unwrap(),
            cert: format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR")),
            blob: "10Mbit".to_string(),
            connections: 2,
            streams: 2,
            duration: None,
        }
    }
}

#[derive(Parser, Clone, Debug)]
#[clap(name = "server")]
pub struct ServerArgs {
    /// TLS private key in PEM format
    #[clap(short, long, requires = "cert")]
    pub key: String,
    /// TLS certificate in PEM format
    #[clap(short, long, requires = "key")]
    pub cert: String,
    /// Address to listen on
    #[clap(default_value = "0.0.0.0:4433")]
    pub listen: SocketAddr,
}

impl ServerArgs {
    pub fn test() -> Self {
        ServerArgs {
            key: format!("{}/../res/pem/key.pem", env!("CARGO_MANIFEST_DIR")),
            cert: format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR")),
            listen: "127.0.0.1:4433".parse().unwrap(),
        }
    }
}

pub trait Client
where
    Self: Sized,
{
    fn new(args: ClientArgs) -> Result<Self>;
    fn connect(&mut self) -> impl Future<Output = Result<()>>;
    /// Runs the requests of this connection and returns their latencies.
    fn run(&mut self) -> impl Future<Output = Result<Vec<Duration>>>;
}

pub trait Server
where
    Self: Sized,
{
    fn new(args: ServerArgs) -> Result<Self>;
    fn listen(&mut self) -> impl Future<Output = Result<()>>;
}
