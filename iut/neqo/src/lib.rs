mod client;
mod server;

pub use client::Client;
pub use server::Server;

use std::{io, net::SocketAddr};

use anyhow::{Context, Result};
use common::bind_socket;
use neqo_common::datagram;
use neqo_transport::{ConnectionParameters, StreamType};
use neqo_udp::{DatagramIter, RecvBuf};
use quinn_udp::UdpSocketState;
use tracing::debug;
use utils::perf::{CONNECTION_WINDOW, IDLE_TIMEOUT, MAX_STREAMS, STREAM_WINDOW};

/// Connection parameters shared by client and server (see docs/PROTOCOL.md).
pub(crate) fn connection_parameters() -> ConnectionParameters {
    ConnectionParameters::default()
        .idle_timeout(IDLE_TIMEOUT)
        .max_data(CONNECTION_WINDOW.into())
        .max_stream_data(StreamType::BiDi, false, STREAM_WINDOW.into())
        .max_stream_data(StreamType::BiDi, true, STREAM_WINDOW.into())
        .max_streams(StreamType::BiDi, MAX_STREAMS)
}

/// A UDP socket with GSO/GRO support via quinn-udp.
///
/// Mirrors the `Socket` type in neqo-bin, which cannot live in neqo-udp itself
/// because of cargo-vet constraints on the tokio dependency in Firefox.
pub(crate) struct UdpSocket {
    state: UdpSocketState,
    inner: tokio::net::UdpSocket,
}

impl UdpSocket {
    /// Bind to `addr` and initialise the quinn-udp socket state.
    pub(crate) fn bind(addr: SocketAddr) -> Result<(Self, SocketAddr)> {
        let std_socket = bind_socket(addr)?;
        let state = UdpSocketState::new((&std_socket).into()).context("init UdpSocketState")?;
        let inner = tokio::net::UdpSocket::from_std(std_socket)?;
        let local_addr = inner.local_addr()?;
        Ok((Self { state, inner }, local_addr))
    }

    pub(crate) async fn readable(&self) -> io::Result<()> {
        self.inner.readable().await
    }

    /// Send a datagram batch, waiting while the OS send buffer is full.
    pub(crate) async fn send(&self, d: &datagram::Batch) -> io::Result<()> {
        loop {
            let res = self.inner.try_io(tokio::io::Interest::WRITABLE, || {
                neqo_udp::send_inner(&self.state, (&self.inner).into(), d)
            });
            match res {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.inner.writable().await?,
                res => return res,
            }
        }
    }

    /// Receive a batch of datagrams. Returns `Ok(None)` when no data is ready.
    pub(crate) fn recv<'a>(
        &self,
        local_addr: SocketAddr,
        recv_buf: &'a mut RecvBuf,
    ) -> io::Result<Option<DatagramIter<'a>>> {
        self.inner
            .try_io(tokio::io::Interest::READABLE, || {
                neqo_udp::recv_inner(local_addr, &self.state, &self.inner, recv_buf)
            })
            .map(Some)
            .or_else(|e| {
                if e.kind() == io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(e)
                }
            })
    }

    /// Maximum number of GSO segments the OS will accept in a single sendmsg.
    pub(crate) fn max_gso_segments(&self) -> usize {
        self.state.max_gso_segments()
    }
}

/// Initialize NSS crypto with a certificate database.
pub(crate) fn init_crypto_db(db_path: &str) -> Result<()> {
    nss::init_db(std::path::Path::new(db_path)).context("failed to initialize NSS crypto database")
}

/// Initialize the NSS crypto database from the canonical bundled path.
pub(crate) fn init_default_crypto_db() -> Result<()> {
    let path = format!("{}/../../res/nssdb", env!("CARGO_MANIFEST_DIR"));
    debug!("initializing NSS certificate database from {path}");
    init_crypto_db(&path)
}
