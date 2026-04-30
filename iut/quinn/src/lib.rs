pub(crate) mod backend {
    pub(crate) use quinn::{ClientConfig, Connection, ConnectionError, Endpoint, Incoming, RecvStream, SendStream, ServerConfig, TokioRuntime};
    pub(crate) use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
}

pub(crate) const CLIENT_TARGET: &str = "quinn::client";
pub(crate) const SERVER_TARGET: &str = "quinn::server";

pub(crate) fn setup_qlog_transport(dir: &str, role: &str) -> anyhow::Result<quinn::TransportConfig> {
    let path = std::path::Path::new(dir).join(format!("{role}.sqlog"));
    let file = std::fs::File::create(&path)?;
    let mut transport = quinn::TransportConfig::default();
    let mut qlog_cfg = quinn::QlogConfig::default();
    qlog_cfg.writer(Box::new(file));
    transport.qlog_stream(qlog_cfg.into_stream());
    Ok(transport)
}

#[path = "../../common/quinn-noq/mod.rs"]
mod common;

pub use common::{Client, Server};
