pub(crate) mod backend {
    pub(crate) use quinn::{ClientConfig, Connection, ConnectionError, Endpoint, Incoming, RecvStream, SendStream, ServerConfig, TokioRuntime};
    pub(crate) use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
}

pub(crate) const CLIENT_TARGET: &str = "quinn::client";
pub(crate) const SERVER_TARGET: &str = "quinn::server";

#[path = "../../common/quinn-noq/mod.rs"]
mod common;

pub use common::{Client, Server};
