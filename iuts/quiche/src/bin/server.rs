use log::{debug, error, info, warn};
use quiche_iut::{Endpoint, Incoming, QcHandle};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use utils::io::{
    Callbacks, ConnectionDecision, ConnectionHandle, HLQuicEndpoint, QuicConfig, QuicEndpoint,
};

#[derive(Clone)]
enum ConnectionData {
    Ready(ReadyData),
    Waiting(WaitingData),
    Failed,
}

#[derive(Clone)]
struct WaitingData {
    received: usize,
    bytes: [u8; 8],
}

#[derive(Clone)]
struct ReadyData {
    to_send: usize,
    sent: usize,
}

type ConnMap = Option<HashMap<QcHandle, ConnectionData>>;
static CONNECTION_MAP: Mutex<ConnMap> = Mutex::new(None);

fn main() {
    env_logger::init();
    init_conn_map();
    let callbacks = Callbacks {
        incoming_connection: Arc::new(handle_inc),
        new_connection: Arc::new(handle_conn),
        new_data: Arc::new(new_data),
        writable: Arc::new(writable),
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
    info!("new connection!");
    new_data(handle);
}

fn init_conn_map() {
    let mut m = CONNECTION_MAP.lock().unwrap();
    *m = Some(HashMap::new());
}

fn insert<'a>(guard: &mut MutexGuard<'a, ConnMap>, handle: &QcHandle, state: ConnectionData) {
    let m = match &mut **guard {
        Some(a) => a,
        None => panic!("conn map not initialised"),
    };
    m.insert(handle.clone(), state);
}

fn get<'a>(guard: &mut MutexGuard<'a, ConnMap>, handle: &QcHandle) -> Option<ConnectionData> {
    let m = match &mut **guard {
        Some(a) => a,
        None => panic!("conn map not initialised"),
    };
    m.get(handle).cloned()
}

fn update<'a>(guard: &mut MutexGuard<'a, ConnMap>, handle: &QcHandle, state: ConnectionData) {
    let mut m = match &mut **guard {
        Some(a) => a,
        None => panic!("conn map not initialised"),
    };
    if let Some(d) = m.get_mut(&handle) {
        *d = state;
    } else {
        m.insert(handle.clone(), state);
    }
}

fn update_fail<'a>(guard: &mut MutexGuard<'a, ConnMap>, handle: &QcHandle) {
    let state = ConnectionData::Failed;
    update(guard, handle, state)
}

fn get_guard() -> MutexGuard<'static, ConnMap> {
    CONNECTION_MAP.lock().unwrap()
}

fn new_data(handle: QcHandle) {
    let mut buf = [0; 4096];
    while let Some(s) = handle.clone().readable_stream() {
        if let Some((len, fin)) = handle.read(s, &mut buf) {
            let mut guard = get_guard();
            let mut prev = match get(&mut guard, &handle) {
                None => WaitingData {
                    received: 0,
                    bytes: [0; 8],
                },
                Some(ConnectionData::Waiting(w)) => w,
                Some(_) => {
                    error!("data after fail or ready");
                    update_fail(&mut guard, &handle);
                    continue;
                }
            };
            let new_len = len + prev.received;
            if new_len > 8 {
                error!("more than 8 bytes!");
                update_fail(&mut guard, &handle);
                continue;
            }
            // Read new bytes into local buffer
            for i in 0..len {
                prev.bytes[prev.received + i] = buf[i];
            }
            if fin == false {
                info!("partial packet received, waiting for more data");
                let new_data = WaitingData {
                    received: new_len,
                    bytes: prev.bytes,
                };
                update(&mut guard, &handle, ConnectionData::Waiting(new_data));
                continue;
            }
            if new_len != 8 {
                error!("expected one 64bit number as a request");
                update_fail(&mut guard, &handle);
                continue;
            }
            let to_send =
                usize::from_be_bytes(prev.bytes.try_into().expect("unexpected slice size"));
            update(
                &mut guard,
                &handle,
                ConnectionData::Ready(ReadyData { to_send, sent: 0 }),
            );
            write_stream(&mut guard, &handle, s);
        }
    }
}

fn send_until_full(buf: &[u8], handle: QcHandle, s: u64, data: &ReadyData) -> usize {
    let rem = data.to_send - data.sent;
    let (fin, size) = if buf.len() >= rem {
        (true, rem)
    } else {
        (false, buf.len())
    };
    let written = match handle.write(s, &buf[..size], fin) {
        Some(n) => n,
        None => return 0,
    };
    /*
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
    */
    info!("wrote {}", written);
    written
}

fn write_stream<'a>(guard: &mut MutexGuard<'a, ConnMap>, handle: &QcHandle, s: u64) {
    let data = match get(guard, handle) {
        Some(ConnectionData::Ready(a)) => a,
        _ => {
            warn!("attempt to send on uninitialized handle...");
            return;
        }
    };
    let written = send_until_full(&WRITE_DATA, handle.clone(), s, &data);
    let new_data = ReadyData {
        to_send: data.to_send,
        sent: written + data.sent,
    };
    debug!("sent {} bytes ({} - {})", written, data.sent, new_data.sent);
    update(guard, handle, ConnectionData::Ready(new_data));
}

static WRITE_DATA: [u8; 1024 * 1024] = [0; 1024 * 1024];
fn writable(handle: QcHandle) {
    while let Some(s) = handle.writable_stream() {
        let mut guard = get_guard();
        write_stream(&mut guard, &handle, s);
    }
}
