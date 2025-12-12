use anyhow::Result;
use clap::Parser;
use std::{future::Future, net::SocketAddr};
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
}

impl ClientArgs {
    pub fn test() -> Self {
        ClientArgs {
            url: Url::parse("https://127.0.0.1:4433").unwrap(),
            unencrypted: false,
            cert: format!("{}/../res/pem/cert.pem", env!("CARGO_MANIFEST_DIR")),
            blob: "50Mbit".to_string(),
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
    #[clap(default_value = "0.0.0.0:4433")]
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

pub trait Client
where
    Self: Sized,
{
    fn new(args: ClientArgs) -> Result<Self>;
    fn connect(&mut self) -> impl Future<Output = Result<()>>;
    fn run(&mut self) -> impl Future<Output = Result<()>>;
}

pub trait Server
where
    Self: Sized,
{
    fn new(args: ServerArgs) -> Result<Self>;
    fn listen(&mut self) -> impl Future<Output = Result<()>>;
}
