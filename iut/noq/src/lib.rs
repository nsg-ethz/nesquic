pub(crate) mod backend {
    pub(crate) use noq::{ClientConfig, Connection, ConnectionError, Endpoint, Incoming, RecvStream, SendStream, ServerConfig, TokioRuntime};
    pub(crate) use noq::crypto::rustls::{QuicClientConfig, QuicServerConfig};
}

pub(crate) const CLIENT_TARGET: &str = "noq::client";
pub(crate) const SERVER_TARGET: &str = "noq::server";

#[path = "../../common/quinn-noq/mod.rs"]
mod common;

pub use common::{Client, Server};
