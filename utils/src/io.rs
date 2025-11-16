pub use anyhow::{bail, Result};
use log::debug;
pub use std::{io, net::SocketAddr, time};
pub use tokio::net::UdpSocket;

pub struct Transmit<'a> {
    pub to: SocketAddr,
    pub data: &'a [u8],
}

pub enum DatagramEvent<'a, ID> {
    NewConnection(ID),
    Known(ID),
    Respond(Transmit<'a>),
}

// Implicitly locks
pub trait Connection: Sized + Sync + Send {
    fn write(&mut self, stream_id: u64, data: &[u8], fin: bool) -> Option<usize>;
    fn read(&mut self, stream_id: u64, buf: &mut [u8]) -> Option<(usize, bool)>;
    fn close(&mut self);
    fn is_closed(&mut self) -> bool;
    // If false, calling any other function will UB
    fn is_cleaned(&mut self) -> bool;
    fn poll(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)>;

    fn readable_stream(&mut self) -> Option<u64>;
    fn writable_stream(&mut self) -> Option<u64>;

    fn time_to_timeout(&mut self) -> Option<time::Duration>;
    // MUST check if there actually is a timeout
    fn on_timeout(&mut self);

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

// #[async_trait]
// pub trait Endpoint {
//     type INC: Send + Sync;
//     type CH: Connection;

//     async fn bind(&self, addr: SocketAddr) -> Result<UdpSocket> {
//         let socket = UdpSocket::bind(addr).await.unwrap();
//         Ok(socket)
//     }

//     async fn event_loop(&mut self, socket: UdpSocket) -> Result<()> {
//         let addr = socket.local_addr()?;
//         let mut buf = [0; 65535];
//         let mut out = [0; 1350]; // TODO: MAX_DATAGRAM_SIZE here
//         'socket: loop {
//             let (len, from) = match socket.try_recv_from(&mut buf) {
//                 Ok(tuple) => tuple,
//                 Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
//                     self.clean();
//                     socket.recv_from(&mut buf).await.expect("recv error")
//                 }
//                 Err(e) => bail!(e),
//             };
//             //let (len, from) = socket.recv_from(&mut buf).await.expect("recv error");
//             let pkt_buf = &mut buf[..len];
//             let Some(event) = self.handle_packet(pkt_buf, from) else {
//                 // Invalid packet
//                 continue 'socket;
//             };
//             debug!("received {} bytes from {}", len, from);

//             match event {
//                 DatagramEvent::NewConnection(id) => {
//                     match self.incoming_connection(&id) {
//                         ConnectionDecision::Accept => {
//                             self.accept_connection(&id, from)?;

//                             self.conn_recv(&id, pkt_buf, from, addr);
//                             self.start_timeout_timer(&id);
//                             self.new_connection(&id);
//                         }
//                         ConnectionDecision::Reject => {
//                             self.reject_connection(id, from);
//                         }
//                     };
//                 }
//                 DatagramEvent::Known(id) => {
//                     self.conn_recv(&id, pkt_buf, from, addr);
//                     self.new_data(&id);
//                 }
//                 DatagramEvent::Respond(t) => {
//                     let mut sent = 0;
//                     let to_send = t.data.len();
//                     while sent < to_send {
//                         sent += socket
//                             .send_to(&t.data[sent..], t.to)
//                             .await
//                             .expect("send error");
//                     }
//                 }
//             };
//             //self.send_all(&mut out).await;
//         }
//     }

//     fn handle_packet(
//         &self,
//         pkt_buf: &mut [u8],
//         from: SocketAddr,
//     ) -> Option<DatagramEvent<Self::INC>>;

//     fn clean(&mut self);

//     fn accept_connection(&mut self, _: &Self::INC, _: SocketAddr) -> Result<()> {
//         bail!("Default implementation always fails")
//     }

//     fn reject_connection(&mut self, _: Self::INC, _: SocketAddr) {}

//     // fn send_all(&mut self, out: &mut [u8], socket: &UdpSocket) {
//     //     // let socket = self.socket();
//     //     let mut cxs = self.connections_vec().clone();
//     //     for conn in cxs.iter_mut() {
//     //         drain_conn(out, conn, &socket);
//     //     }

//     //     self.clean();
//     //     /*
//     //     for handle in self.ep.connections_vec() {
//     //         (self.writable)(handle.clone());
//     //     }
//     //     */
//     // }

//     fn new_connection(&mut self, id: &Self::INC);

//     fn new_data(&mut self, id: &Self::INC);

//     fn incoming_connection(&self, _: &Self::INC) -> ConnectionDecision {
//         ConnectionDecision::Accept
//     }

//     fn conn_recv(
//         &mut self,
//         id: &Self::INC,
//         buf: &mut [u8],
//         from: SocketAddr,
//         to: SocketAddr,
//     ) -> Option<usize>;

//     fn start_timeout_timer(&self, id: &Self::INC) {
//         // let m_h = handle.clone();
//         // let socket = self.socket();
//         // tokio::spawn(async move {
//         /*
//         loop {
//             let handle = m_h.clone();
//             let d = match handle.time_to_timeout() {
//                 Some(d) => d,
//                 None => time::Duration::from_millis(1000),
//             };
//             sleep(d).await;
//             info!("Potential timeout!");
//             m_h.on_timeout();
//             let mut buf = [0; 8192];
//             drain_conn(&mut buf, handle.clone(), socket.clone());
//             if handle.is_closed() {
//                 break;
//             }
//         }
//         */
//         // });
//     }
// }

// fn drain_conn<CH: Connection>(buf: &mut [u8], conn: &mut CH, socket: &UdpSocket) {
//     while let Some((len, to)) = conn.poll(buf) {
//         let written = match socket.try_send_to(&buf[..len], to) {
//             Ok(n) => n,
//             Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
//                 break;
//             }
//             Err(e) => {
//                 panic!("send failed: {}", e);
//             }
//         };
//         if written != len {
//             panic!("could not write whole packet");
//         }
//         //socket.send_to(&buf[..len], to).await.expect("send error");
//     }
// }

// /// Relevant for APPLICATION
// pub enum ConnectionDecision {
//     Accept,
//     Reject,
//     // TODO: Retry
// }
