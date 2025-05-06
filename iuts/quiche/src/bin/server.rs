use common::io::{
    Callbacks, ConnectionDecision, ConnectionHandle, HLQuicEndpoint, QuicConfig, QuicEndpoint,
};
use quiche_iut::{Endpoint, Incoming, QcHandle};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone)]
struct ConnectionData {
    to_send: usize,
    sent: usize,
}

static CONNECTION_MAP: Mutex<Option<HashMap<QcHandle, ConnectionData>>> = Mutex::new(None);

fn main() {
    env_logger::init();

    init_conn_map();
    let callbacks = Callbacks {
        incoming_connection: Box::new(handle_inc),
        new_connection: Box::new(handle_conn),
        new_data: Box::new(new_data),
        writable: Box::new(writable),
    };
    let cfg = QuicConfig::new();
    let Some(endpoint) = Endpoint::with_config(cfg) else {
        panic!("invalid config");
    };
    let mut endpoint = HLQuicEndpoint::new(endpoint, callbacks);
    let _ = endpoint.listen();
}

fn handle_inc(_: &Incoming) -> ConnectionDecision {
    return ConnectionDecision::Accept;
}
fn handle_conn(handle: QcHandle) {
    new_data(handle);
}

fn init_conn_map() {
    let mut m = CONNECTION_MAP.lock().unwrap();
    *m = Some(HashMap::new());
}

fn insert(handle: QcHandle, to_send: usize) {
    let mut guard = CONNECTION_MAP.lock().unwrap();
    let mut m = match &mut *guard {
        Some(a) => a,
        None => panic!("conn map not initialised"),
    };
    m.insert(handle, ConnectionData { to_send, sent: 0 });
}

fn get(handle: QcHandle) -> Option<ConnectionData> {
    let mut guard = CONNECTION_MAP.lock().unwrap();
    let mut m = match &mut *guard {
        Some(a) => a,
        None => panic!("conn map not initialised"),
    };
    m.get(&handle).cloned()
}

fn update(handle: QcHandle, data: ConnectionData) {
    let mut guard = CONNECTION_MAP.lock().unwrap();
    let mut m = match &mut *guard {
        Some(a) => a,
        None => panic!("conn map not initialised"),
    };
    if let Some(d) = m.get_mut(&handle) {
        *d = data;
    } else {
        m.insert(handle, data);
    }
}

fn new_data(handle: QcHandle) {
    let mut buf = [0; 4096];
    while let Some(s) = handle.clone().readable_stream() {
        if let Some((len, fin)) = handle.read(s, &mut buf) {
            if fin != true || len != 8 {
                continue;
            }
            let to_send = usize::from_be_bytes(buf[..8].try_into().expect("unexpected slice size"));
            update(handle.clone(), ConnectionData { to_send, sent: 0 });
        }
    }
    writable(handle);
}

fn send_until_full(buf: &[u8], handle: QcHandle, s: u64, data: &ConnectionData) -> usize {
    let to_send = data.to_send;
    let mut sent = data.sent;
    loop {
        let rem = to_send - sent;
        let (fin, size) = if buf.len() >= rem {
            (true, rem)
        } else {
            (false, buf.len())
        };
        match handle.write(s, &buf[..size], fin) {
            Some(n) => sent += n,
            None => break,
        };
        if fin {
            break;
        }
    }
    sent
}

const WRITE_DATA: [u8; 1024 * 1024] = [0; 1024 * 1024];
fn writable(handle: QcHandle) {
    while let Some(s) = handle.writable_stream() {
        let data = match get(handle.clone()) {
            Some(a) => a,
            None => {
                return;
            }
        };
        let sent = send_until_full(&WRITE_DATA, handle.clone(), s, &data);
        let new_data = ConnectionData {
            to_send: data.to_send,
            sent,
        };
        update(handle.clone(), new_data);
        //println!("sent {} bytes ({} - {})", sent - data.sent, data.sent, sent);
    }
}
