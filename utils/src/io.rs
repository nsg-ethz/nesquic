use log::debug;
use std::io;
use std::marker::PhantomData;
pub use std::net::SocketAddr;
use std::sync::Arc;
use std::time;
pub use tokio::net::UdpSocket;

pub struct Transmit<'a> {
    pub to: SocketAddr,
    pub data: &'a [u8],
}

pub enum DatagramEvent<'a, INC, CH: ConnectionHandle> {
    NewConnection(INC),
    Known(CH),
    Respond(Transmit<'a>),
}

pub struct QuicConfig<'a> {
    pub key_fp: &'a str,
    pub cert_fp: &'a str,
    pub local_addr_str: &'a str,
    pub max_recv_datagram_size: usize,
    pub max_send_datagram_size: usize,
}

impl QuicConfig<'_> {
    pub fn new() -> Self {
        QuicConfig {
            key_fp: "res/pem/key.pem",
            cert_fp: "res/pem/cert.pem",
            local_addr_str: "127.0.0.1:4433",
            max_recv_datagram_size: 1350,
            max_send_datagram_size: 1350,
        }
    }
}

// Implicitly locks
pub trait ConnectionHandle: Sized + Sync + Send + Clone {
    fn write(&self, stream_id: u64, data: &[u8], fin: bool) -> Option<usize>;
    fn read(&self, stream_id: u64, buf: &mut [u8]) -> Option<(usize, bool)>;
    fn close(&self);
    fn is_closed(&self) -> bool;
    // If false, calling any other function will UB
    fn is_cleaned(&self) -> bool;
    fn poll(&self, buf: &mut [u8]) -> Option<(usize, SocketAddr)>;

    fn readable_stream(&self) -> Option<u64>;
    fn writable_stream(&self) -> Option<u64>;

    fn time_to_timeout(&self) -> Option<time::Duration>;
    // MUST check if there actually is a timeout
    fn on_timeout(&self);

    /*
    async fn wait_timeout(&mut self) {
        while !self.is_closed() {
            let duration = match self.time_to_timeout() {
                Some(d) => d,
                // TODO: This is supposed to wait indefinitely, but if the timeout
                // duration is changed, this function should know that and timeout
                // at the right time. For now, just wait an arbitrary (quite short)
                // time and check for timeouts
                None => time::Duration::from_millis(100),
            };
            sleep(duration).await;
            self.on_timeout();
        }
    }
    */
}

pub struct Callbacks<INC, CH: ConnectionHandle> {
    pub incoming_connection: Arc<dyn Fn(&INC) -> ConnectionDecision + Send + Sync>,
    pub new_connection: Arc<dyn Fn(CH) + Send + Sync>,
    pub new_data: Arc<dyn Fn(CH) + Send + Sync>,
    pub writable: Arc<dyn Fn(CH) + Send + Sync>,
}

pub struct HLQuicEndpoint<INC: Send + Sync, CH: ConnectionHandle, QE: QuicEndpoint<INC, CH>> {
    ep: Box<QE>, //low level end point
    socket: Option<Arc<UdpSocket>>,
    callbacks: Callbacks<INC, CH>,
    inc: PhantomData<INC>,
    ch: PhantomData<CH>,
}

impl<
        INC: Send + Sync + 'static,
        CH: ConnectionHandle + 'static,
        QE: QuicEndpoint<INC, CH> + 'static,
    > HLQuicEndpoint<INC, CH, QE>
{
    pub fn new(ep: Box<QE>, callbacks: Callbacks<INC, CH>) -> Self {
        HLQuicEndpoint {
            ep,
            socket: None,
            callbacks,
            inc: PhantomData,
            ch: PhantomData,
        }
    }
    async fn read_loop(&self) -> ! {
        let mut buf = [0; 65535];
        let mut out = [0; 1350]; // TODO: MAX_DATAGRAM_SIZE here
        let socket = self.socket();
        'socket: loop {
            let (len, from) = match socket.try_recv_from(&mut buf) {
                Ok(tuple) => tuple,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.send_all(&mut out);
                    socket.recv_from(&mut buf).await.expect("recv error")
                }
                Err(e) => {
                    panic!("recv error: {}", e)
                }
            };
            //let (len, from) = socket.recv_from(&mut buf).await.expect("recv error");
            let pkt_buf = &mut buf[..len];
            let Some(event) = self.ep.handle_packet(pkt_buf, from) else {
                // Invalid packet
                continue 'socket;
            };
            debug!("received {} bytes from {}", len, from);
            use DatagramEvent::*;
            match event {
                NewConnection(incoming) => {
                    match (self.callbacks.incoming_connection)(&incoming) {
                        ConnectionDecision::Accept => {
                            if let Some(handle) = self.ep.accept_connection(incoming, from) {
                                self.ep.conn_handle(handle.clone(), pkt_buf, from);
                                self.start_timeout_timer(handle.clone());
                                (self.callbacks.new_connection)(handle);
                            }
                        }
                        ConnectionDecision::Reject => {
                            self.ep.reject_connection(incoming, from);
                        }
                    };
                }
                Known(conn) => {
                    self.ep.conn_handle(conn.clone(), pkt_buf, from);
                    (self.callbacks.new_data)(conn.clone());
                    (self.callbacks.writable)(conn);
                }
                Respond(t) => {
                    let mut sent = 0;
                    let to_send = t.data.len();
                    while sent < to_send {
                        sent += socket
                            .send_to(&t.data[sent..], t.to)
                            .await
                            .expect("send error");
                    }
                }
            };
            //self.send_all(&mut out).await;
        }
    }
    //#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
    #[tokio::main(flavor = "current_thread")]
    pub async fn listen(&mut self) -> ! {
        let socket_addr = self.ep.get_local_addr();
        let socket = UdpSocket::bind(socket_addr).await.unwrap();
        let socket = Arc::new(socket);
        self.socket = Some(socket);
        /*
        const NUM_THREADS: usize = 5;
        let self_unmut = Arc::new(&*self);
        for _ in 0..NUM_THREADS-1 {
            let this = unsafe { std::mem::transmute::<Arc<&Self>, Arc<&'static Self>>(self_unmut.clone()) };
            tokio::spawn(async move {
                this.read_loop().await;
            });
        }
        */
        self.read_loop().await;
    }

    fn socket(&self) -> Arc<UdpSocket> {
        match &self.socket {
            Some(s) => s.clone(),
            None => panic!("NO SOCKET"),
        }
    }

    fn send_all(&self, out: &mut [u8]) {
        let socket = self.socket();
        let cxs = self.ep.connections_vec();
        for handle in cxs {
            let handle = handle.clone();
            drain_handle(out, handle.clone(), socket.clone());
        }

        self.ep.clean();
        /*
        for handle in self.ep.connections_vec() {
            (self.callbacks.writable)(handle.clone());
        }
        */
    }

    fn start_timeout_timer(&self, handle: CH) {
        let m_h = handle.clone();
        let socket = self.socket();
        tokio::spawn(async move {
            /*
            loop {
                let handle = m_h.clone();
                let d = match handle.time_to_timeout() {
                    Some(d) => d,
                    None => time::Duration::from_millis(1000),
                };
                sleep(d).await;
                info!("Potential timeout!");
                m_h.on_timeout();
                let mut buf = [0; 8192];
                drain_handle(&mut buf, handle.clone(), socket.clone());
                if handle.is_closed() {
                    break;
                }
            }
            */
        });
    }
}

fn drain_handle<CH: ConnectionHandle>(buf: &mut [u8], handle: CH, socket: Arc<UdpSocket>) {
    while let Some((len, to)) = handle.poll(buf) {
        let written = match socket.try_send_to(&buf[..len], to) {
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                break;
            }
            Err(e) => {
                panic!("send failed: {}", e);
            }
        };
        if written != len {
            panic!("could not write whole packet");
        }
        //socket.send_to(&buf[..len], to).await.expect("send error");
    }
}

/// Implemented by TRANSLATION
pub trait QuicEndpoint<INC: Sync + Send, CH: ConnectionHandle>: Sync + Send {
    fn get_local_addr(&self) -> &SocketAddr;
    fn with_config(cfg: QuicConfig<'_>) -> Option<Box<Self>>;
    fn handle_packet(&self, pkt_buf: &mut [u8], from: SocketAddr)
        -> Option<DatagramEvent<INC, CH>>;
    fn conn_handle(&self, handle: CH, pkt_buf: &mut [u8], from: SocketAddr) -> Option<usize>;
    fn connections_vec(&self) -> Vec<CH>;
    fn clean(&self);
    fn accept_connection(&self, inc: INC, from: SocketAddr) -> Option<CH>;
    fn reject_connection(&self, inc: INC, from: SocketAddr);
}

/// Relevant for APPLICATION
pub enum ConnectionDecision {
    Accept,
    Reject,
    // TODO: Retry
}
