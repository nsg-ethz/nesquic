use crate::{Connection, ConnectionData, ConnectionId};
use anyhow::bail;
use async_trait::async_trait;
use log::{error, info, warn};
use quiche::Config;
use ring::{hmac::Key, rand};
use std::collections::HashMap;
use utils::{
    bin,
    bin::ServerArgs,
    io::{Connection as IoConnection, DatagramEvent, Endpoint, Result, SocketAddr},
};

pub struct Server {
    args: ServerArgs,
    local_addr: SocketAddr,
    connections: HashMap<ConnectionId, Connection>,
    config: Config,
    conn_id_seed: Key,
}

#[async_trait]
impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        let mut config = Config::new(quiche::PROTOCOL_VERSION)?;
        config.load_cert_chain_from_pem_file(&args.cert)?;
        config.set_application_protos(&[b"perf"])?;

        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(1350);
        config.set_max_send_udp_payload_size(1350);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);
        config.enable_early_data();

        let rng = rand::SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();

        let local_addr = "127.0.0.1:0".parse()?;

        Ok(Server {
            args,
            local_addr,
            connections: HashMap::new(),
            conn_id_seed,
            config,
        })
    }

    async fn listen(&mut self) -> Result<()> {
        let socket = self.bind(self.local_addr).await?;
        self.event_loop(socket).await
    }
}

#[async_trait]
impl Endpoint for Server {
    type INC = ConnectionId;
    type CH = Connection;

    fn accept_connection(&mut self, id: &ConnectionId, from: SocketAddr) -> Result<()> {
        let conn = match quiche::accept(&id, None, self.local_addr, from, &mut self.config) {
            Ok(c) => c,
            Err(e) => {
                bail!("quiche did not accept: {}", e);
            }
        };

        let conn = Connection::new(conn, ConnectionData::default());
        self.connections.insert(id.clone(), conn);
        Ok(())
    }

    fn new_connection(&mut self, id: &ConnectionId) {
        self.new_data(id);
    }

    fn new_data(&mut self, id: &ConnectionId) {
        let conn = self.connections.get_mut(id).unwrap();

        let mut buf = [0; 4096];
        while let Some(s) = conn.readable_stream() {
            if let Some((len, fin)) = conn.read(s, &mut buf) {
                if fin != true || len != 8 {
                    println!("expected one 64bit number as a request");
                    continue;
                }
                let to_send =
                    usize::from_be_bytes(buf[..8].try_into().expect("unexpected slice size"));
                conn.data.to_send = to_send;
            }
        }

        const WRITE_DATA: [u8; 1024 * 1024] = [0; 1024 * 1024];
        while let Some(s) = conn.writable_stream() {
            let to_send = conn.data.to_send;
            let mut sent = conn.data.sent;
            loop {
                let rem = to_send - sent;
                let (fin, size) = if WRITE_DATA.len() >= rem {
                    (true, rem)
                } else {
                    (false, WRITE_DATA.len())
                };
                match conn.write(s, &WRITE_DATA[..size], fin) {
                    Some(n) => sent += n,
                    None => break,
                };
                if fin {
                    break;
                }
            }

            conn.data.sent = sent;
        }
    }

    fn conn_recv(
        &mut self,
        id: &ConnectionId,
        buf: &mut [u8],
        from: SocketAddr,
        to: SocketAddr,
    ) -> Option<usize> {
        let conn = self.connections.get_mut(id).unwrap();
        let recv_info = quiche::RecvInfo { to, from };
        match conn.inner.recv(buf, recv_info) {
            Ok(n) => Some(n),
            Err(e) => {
                error!("conn handle failed: {:?}", e);
                None
            }
        }
    }

    fn handle_packet(
        &self,
        pkt_buf: &mut [u8],
        _from: SocketAddr,
    ) -> Option<DatagramEvent<ConnectionId>> {
        let hdr = match quiche::Header::from_slice(pkt_buf, quiche::MAX_CONN_ID_LEN) {
            Ok(v) => v,
            Err(_) => {
                return None;
            }
        };
        let conn_id = ring::hmac::sign(&self.conn_id_seed, &hdr.dcid);
        let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
        let conn_id = conn_id.to_vec().into();

        let known =
            self.connections.contains_key(&hdr.dcid) || self.connections.contains_key(&conn_id);

        if known {
            return Some(DatagramEvent::Known(conn_id));
        }

        if hdr.ty != quiche::Type::Initial {
            warn!("unexpected packet");
            return None;
        }
        if !quiche::version_is_supported(hdr.version) {
            warn!("version negotiation (not implemented)");
            // TODO: version negotiation
            return None;
        }

        Some(DatagramEvent::NewConnection(conn_id))
    }

    fn clean(&mut self) {
        self.connections.retain(|_, conn| {
            if conn.inner.is_closed() {
                info!("cleaned connection");
            }
            !conn.inner.is_closed()
        });
    }
}
