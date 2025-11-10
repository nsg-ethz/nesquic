use std::net::SocketAddr;

use clap::Parser;
use url::Url;

#[derive(Parser, Debug)]
#[clap(name = "client")]
pub struct ClientArgs {
    /// the address of the server
    pub url: Url,

    /// do TLS handshake, but don't encrypt connection
    #[clap(long = "unencrypted")]
    pub unencrypted: bool,

    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert")]
    pub cert: String,

    #[clap(short = 'b', long = "blob")]
    pub blob: String,

    #[clap(short = 'r', long = "reps", default_value = "1")]
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

#[derive(Parser, Debug)]
#[clap(name = "server")]
pub struct ServerArgs {
    /// do TLS handshake, but don't encrypt connection
    #[clap(long = "unencrypted", default_value = "false")]
    pub unencrypted: bool,
    /// TLS private key in PEM format
    #[clap(short = 'k', long = "key", requires = "cert")]
    pub key: String,
    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert", requires = "key")]
    pub cert: String,
    /// Address to listen on
    #[clap(long = "listen", default_value = "127.0.0.1:4433")]
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
