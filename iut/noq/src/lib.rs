pub(crate) mod backend {
    pub(crate) use noq::{ClientConfig, Connection, ConnectionError, Endpoint, Incoming, RecvStream, SendStream, ServerConfig, TokioRuntime};
    pub(crate) use noq::crypto::rustls::{QuicClientConfig, QuicServerConfig};
}

pub(crate) const CLIENT_TARGET: &str = "noq::client";
pub(crate) const SERVER_TARGET: &str = "noq::server";

pub(crate) fn setup_qlog_transport(dir: &str, _role: &str) -> anyhow::Result<noq::TransportConfig> {
    let mut transport = noq::TransportConfig::default();
    transport.qlog_from_path(std::path::Path::new(dir), "");
    Ok(transport)
}

#[path = "../../common/quinn-noq/mod.rs"]
mod common;

pub use common::{Client, Server};
