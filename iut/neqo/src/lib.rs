mod client;
mod server;

pub use client::Client;
pub use server::Server;

use std::{io, net::SocketAddr};

use anyhow::{Context, Result};
use neqo_common::datagram;
use neqo_udp::{DatagramIter, RecvBuf};
use quinn_udp::UdpSocketState;
use socket2::{Domain, Protocol, Socket, Type};
use tracing::debug;

/// Create and bind a UDP socket. Enables dual-stack IPv6 when the address is IPv6.
/// Returns a standard std::net::UdpSocket ready to be handed to tokio.
pub(crate) fn bind_socket(addr: SocketAddr) -> Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))
        .context("create socket")?;

    if addr.is_ipv6() {
        socket.set_only_v6(false).context("set_only_v6")?;
    }

    socket
        .bind(&socket2::SockAddr::from(addr))
        .context("binding socket")?;

    socket
        .set_nonblocking(true)
        .context("set_nonblocking")?;

    Ok(socket.into())
}

/// Bind a socket and wrap it in a Tokio UdpSocket, returning the local address too.
/// Used by the server which does not need GSO support.
pub(crate) fn bind_tokio_socket(
    addr: SocketAddr,
) -> Result<(tokio::net::UdpSocket, SocketAddr)> {
    let socket = tokio::net::UdpSocket::from_std(bind_socket(addr)?)?;
    let local_addr = socket.local_addr()?;
    Ok((socket, local_addr))
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
        let state =
            UdpSocketState::new((&std_socket).into()).context("init UdpSocketState")?;
        let inner = tokio::net::UdpSocket::from_std(std_socket)?;
        let local_addr = inner.local_addr()?;
        Ok((Self { state, inner }, local_addr))
    }

    pub(crate) async fn readable(&self) -> io::Result<()> {
        self.inner.readable().await
    }

    pub(crate) async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await
    }

    /// Send a datagram batch. Returns `WouldBlock` when the OS send buffer is full.
    pub(crate) fn send(&self, d: &datagram::Batch) -> io::Result<()> {
        self.inner.try_io(tokio::io::Interest::WRITABLE, || {
            neqo_udp::send_inner(&self.state, (&self.inner).into(), d)
        })
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
    neqo_crypto::init_db(std::path::Path::new(db_path))
        .context("failed to initialize NSS crypto database")
}

/// Initialize the NSS crypto database from the canonical bundled path.
pub(crate) fn init_default_crypto_db() -> Result<()> {
    let path = format!("{}/../../res/nssdb", env!("CARGO_MANIFEST_DIR"));
    debug!("initializing NSS certificate database from {path}");
    init_crypto_db(&path)
}
