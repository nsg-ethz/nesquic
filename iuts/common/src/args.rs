use std::{
    net::SocketAddr
};

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
}

#[derive(Parser, Debug)]
#[clap(name = "server")]
pub struct ServerArgs {
    /// do TLS handshake, but don't encrypt connection
    #[clap(long = "unencrypted")]
    pub unencrypted: bool,
    /// TLS private key in PEM format
    #[clap(short = 'k', long = "key", requires = "cert")]
    pub key: String,
    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert", requires = "key")]
    pub cert: String,
    /// Address to listen on
    #[clap(long = "listen", default_value = "[::1]:4433")]
    pub listen: SocketAddr,
}