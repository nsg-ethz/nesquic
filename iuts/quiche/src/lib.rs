use log::{error, info, warn};
use quiche;
use ring::rand;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time;
use utils::io::{self, ConnectionHandle, DatagramEvent, QuicConfig, SocketAddr};

pub struct Endpoint {
    cfg: Mutex<quiche::Config>,
    local_addr: SocketAddr,
    conn_id_seed: ring::hmac::Key,
    connections: Mutex<ConnectionMap>,
}

type ConnectionMap = HashMap<quiche::ConnectionId<'static>, QcHandle>;

pub struct Incoming {
    conn_id: quiche::ConnectionId<'static>,
}

pub struct QcHandle {
    conn: Arc<Mutex<quiche::Connection>>,
}

impl QcHandle {
    fn new(c: quiche::Connection) -> QcHandle {
        QcHandle {
            conn: Arc::new(Mutex::new(c)),
        }
    }

    fn get(&self) -> MutexGuard<'_, quiche::Connection> {
        return self.conn.lock().unwrap();
    }
}

impl Hash for QcHandle {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        let ptr = Arc::<_>::as_ptr(&self.conn);
        let num = ptr as usize;
        num.hash(state);
    }
}

impl PartialEq for QcHandle {
    fn eq(&self, other: &QcHandle) -> bool {
        let l = Arc::<_>::as_ptr(&self.conn);
        let r = Arc::<_>::as_ptr(&other.conn);
        l == r
    }
}
impl Eq for QcHandle {}

impl Clone for QcHandle {
    fn clone(&self) -> QcHandle {
        QcHandle {
            conn: self.conn.clone(),
        }
    }
}

impl ConnectionHandle for QcHandle {
    fn write(&self, s: u64, data: &[u8], fin: bool) -> Option<usize> {
        let mut conn = self.get();
        match conn.stream_send(s, data, fin) {
            Ok(s) => Some(s),
            Err(_) => None,
        }
    }
    fn read(&self, s: u64, buf: &mut [u8]) -> Option<(usize, bool)> {
        let mut conn = self.get();
        // NOTE: conn.send should be called immediately after conn.stream_recv
        match conn.stream_recv(s, buf) {
            Ok(s) => Some(s),
            Err(e) => {
                println!("{}", e);
                None
            }
        }
    }
    fn close(&self) {
        let mut conn = self.get();
        conn.close(true, 0x0, b"fail").ok();
    }

    fn is_closed(&self) -> bool {
        let conn = self.get();
        conn.is_closed()
    }

    fn is_cleaned(&self) -> bool {
        self.conn.is_poisoned()
    }

    fn poll(&self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        let mut conn = self.get();
        match conn.send(buf) {
            Ok((l, i)) => Some((l, i.to)),
            Err(quiche::Error::Done) => None,
            Err(e) => {
                println!("{} send failed: {}", conn.trace_id(), e);
                conn.close(false, 0x1, b"fail").ok();
                None
            }
        }
    }

    fn readable_stream(&self) -> Option<u64> {
        let mut conn = self.get();
        conn.stream_readable_next()
    }
    fn writable_stream(&self) -> Option<u64> {
        let mut conn = self.get();
        conn.stream_writable_next()
    }

    fn time_to_timeout(&self) -> Option<time::Duration> {
        let conn = self.get();
        conn.timeout()
    }

    fn on_timeout(&self) {
        let mut conn = self.get();
        conn.on_timeout();
    }
}

impl io::QuicEndpoint<Incoming, QcHandle> for Endpoint {
    fn get_local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }

    fn with_config(cfg: QuicConfig) -> Option<Box<Self>> {
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).ok()?;

        config.load_cert_chain_from_pem_file(cfg.cert_fp).ok()?;
        config.load_priv_key_from_pem_file(cfg.key_fp).ok()?;
        config.set_application_protos(&[b"perf"]).ok()?;

        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(cfg.max_recv_datagram_size);
        config.set_max_send_udp_payload_size(cfg.max_send_datagram_size);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);
        config.enable_early_data();

        let rng = rand::SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).ok()?;

        let connections = ConnectionMap::new();
        Some(Box::new(Endpoint {
            cfg: Mutex::new(config),
            conn_id_seed,
            connections: Mutex::new(connections),
            local_addr: cfg.local_addr_str.parse().expect("invalid local addr"),
        }))
    }

    fn reject_connection(&self, inc: Incoming, from: SocketAddr) {
        _ = from;
        _ = inc;
        return;
    }

    fn accept_connection(&self, inc: Incoming, from: SocketAddr) -> Option<QcHandle> {
        let mut cfg = self.cfg.lock().unwrap();
        let conn = match quiche::accept(&inc.conn_id, None, self.local_addr, from, &mut *cfg) {
            Ok(c) => c,
            Err(e) => {
                error!("quiche did not accept: {}", e);
                return None;
            }
        };
        let handle = QcHandle::new(conn);
        let mut connections = self.connections.lock().unwrap();
        connections.insert(inc.conn_id.clone(), handle.clone());
        Some(handle.clone())
    }

    fn handle_packet(
        &self,
        pkt_buf: &mut [u8],
        _from: SocketAddr,
    ) -> Option<DatagramEvent<Incoming, QcHandle>> {
        let hdr = match quiche::Header::from_slice(pkt_buf, quiche::MAX_CONN_ID_LEN) {
            Ok(v) => v,
            Err(_) => {
                return None;
            }
        };
        let conn_id = ring::hmac::sign(&self.conn_id_seed, &hdr.dcid);
        let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
        let conn_id = conn_id.to_vec().into();

        let connections = self.connections.lock().unwrap();
        if !connections.contains_key(&hdr.dcid) && !connections.contains_key(&conn_id) {
            if hdr.ty != quiche::Type::Initial {
                warn!("unexpected packet");
                return None;
            }
            if !quiche::version_is_supported(hdr.version) {
                warn!("version negotiation (not implemented)");
                // TODO: version negotiation
                return None;
            }
            return Some(DatagramEvent::NewConnection(Incoming { conn_id }));
        } else {
            let handle = if connections.contains_key(&hdr.dcid) {
                connections.get(&hdr.dcid).unwrap()
            } else {
                connections.get(&conn_id).unwrap()
            };
            return Some(DatagramEvent::Known(handle.clone()));
        }
    }

    fn conn_handle(&self, handle: QcHandle, pkt_buf: &mut [u8], from: SocketAddr) -> Option<usize> {
        let mut conn = handle.get();
        let recv_info = quiche::RecvInfo {
            to: self.local_addr,
            from,
        };
        match conn.recv(pkt_buf, recv_info) {
            Ok(n) => Some(n),
            Err(e) => {
                error!("conn handle failed: {:?}", e);
                None
            }
        }
    }

    fn clean(&self) {
        let mut connections = self.connections.lock().unwrap();
        connections.retain(|_, ref mut h| {
            let conn = h.get();
            if conn.is_closed() {
                info!("cleaned connection");
            }
            !conn.is_closed()
        });
    }

    fn connections_vec(&self) -> Vec<QcHandle> {
        let connections = self.connections.lock().unwrap();
        connections.values().map(|a| a.clone()).collect()
    }
}
