use crate::perf::Stats;
use anyhow::Result;
use async_trait::async_trait;
use clap::Parser;
use std::net::SocketAddr;
use url::Url;

#[derive(Parser, Clone, Debug)]
#[clap(name = "client")]
pub struct ClientArgs {
    /// the address of the server
    #[clap(default_value = "https://127.0.0.1:4433")]
    pub url: Url,

    /// do TLS handshake, but don't encrypt connection
    #[clap(long, default_value = "false")]
    pub unencrypted: bool,

    /// TLS certificate in PEM format
    #[clap(short, long)]
    pub cert: String,

    #[clap(short, long)]
    pub blob: String,

    #[clap(short, long, default_value = "1")]
    pub reps: u16,
}

impl ClientArgs {
    pub fn test() -> Self {
        ClientArgs {
            url: Url::parse("https://127.0.0.1:4433").unwrap(),
            unencrypted: false,
            cert: format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR")),
            blob: "100bit".to_string(),
            reps: 1,
        }
    }
}

#[derive(Parser, Clone, Debug)]
#[clap(name = "server")]
pub struct ServerArgs {
    /// do TLS handshake, but don't encrypt connection
    #[clap(long, default_value = "false")]
    pub unencrypted: bool,
    /// TLS private key in PEM format
    #[clap(short, long, requires = "cert")]
    pub key: String,
    /// TLS certificate in PEM format
    #[clap(short, long, requires = "key")]
    pub cert: String,
    /// Address to listen on
    #[clap(default_value = "127.0.0.1:4433")]
    pub listen: SocketAddr,
}

impl ServerArgs {
    pub fn test() -> Self {
        ServerArgs {
            unencrypted: false,
            key: format!("{}/../res/pem/key.pem", env!("CARGO_MANIFEST_DIR")),
            cert: format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR")),
            listen: "127.0.0.1:4433".parse().unwrap(),
        }
    }
}

#[async_trait]
pub trait Client
where
    Self: Sized,
{
    fn new(args: ClientArgs) -> Result<Self>;
    async fn run(&mut self) -> Result<()>;
    fn stats(&self) -> &Stats;
}

#[async_trait]
pub trait Server
where
    Self: Sized,
{
    fn new(args: ServerArgs) -> Result<Self>;
    async fn listen(&self) -> Result<()>;
}
