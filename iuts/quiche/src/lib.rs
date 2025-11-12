use log::error;
use quiche;
use std::{collections::HashMap, time};
use utils::io::{self, SocketAddr};

mod client;
mod server;

pub use client::Client;
pub use server::Server;

pub struct Endpoint {
    cfg: quiche::Config,
    local_addr: SocketAddr,
    conn_id_seed: ring::hmac::Key,
    connections: HashMap<quiche::ConnectionId<'static>, Connection>,
}

type ConnectionId = quiche::ConnectionId<'static>;

#[derive(Clone, Default)]
pub struct ConnectionData {
    to_send: usize,
    sent: usize,
}

pub struct Connection {
    inner: quiche::Connection,
    data: ConnectionData,
}

impl Connection {
    fn new(c: quiche::Connection, data: ConnectionData) -> Connection {
        Connection { inner: c, data }
    }
}

impl io::Connection for Connection {
    fn write(&mut self, s: u64, data: &[u8], fin: bool) -> Option<usize> {
        match self.inner.stream_send(s, data, fin) {
            Ok(s) => Some(s),
            Err(_) => None,
        }
    }

    fn read(&mut self, s: u64, buf: &mut [u8]) -> Option<(usize, bool)> {
        // NOTE: conn.send should be called immediately after conn.stream_recv
        match self.inner.stream_recv(s, buf) {
            Ok(s) => Some(s),
            Err(e) => {
                println!("{}", e);
                None
            }
        }
    }

    fn close(&mut self) {
        self.inner.close(true, 0x0, b"fail").ok();
    }

    fn is_closed(&mut self) -> bool {
        self.inner.is_closed()
    }

    fn is_cleaned(&mut self) -> bool {
        true // TODO: fix
    }

    fn poll(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        match self.inner.send(buf) {
            Ok((l, i)) => Some((l, i.to)),
            Err(quiche::Error::Done) => None,
            Err(e) => {
                println!("{} send failed: {}", self.inner.trace_id(), e);
                self.inner.close(false, 0x1, b"fail").ok();
                None
            }
        }
    }

    fn readable_stream(&mut self) -> Option<u64> {
        self.inner.stream_readable_next()
    }

    fn writable_stream(&mut self) -> Option<u64> {
        self.inner.stream_writable_next()
    }

    fn time_to_timeout(&mut self) -> Option<time::Duration> {
        self.inner.timeout()
    }

    fn on_timeout(&mut self) {
        self.inner.on_timeout();
    }
}
