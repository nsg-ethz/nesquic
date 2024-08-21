use anyhow::{anyhow, Result};
use clap::Parser;
use common::{
    args::ServerArgs, 
    perf::Blob,
    perf::process_req,
};
use io_uring::{cqueue, opcode::{self, Socket}, squeue, types, IoUring};
use log::{
    info,
    error,
    debug
};
use quiche::Connection;
use ring::rand::*;
use slab::Slab;
use std::{
    collections::{HashMap, VecDeque}, net::{
        SocketAddr, UdpSocket
    }, os::fd::{AsRawFd, FromRawFd, RawFd }, str::FromStr, time::Duration, vec
}; 

const BUF_NUM: usize = 64;
const BUF_LEN: usize = 256;
const MAX_DATAGRAM_SIZE: usize = 1350;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    ProvideBuffers { fd: Option<RawFd>, addr: *mut u8, num_bufs: u16, buf_len: i32, bgid: u16, bid: u16 },
    Accept { fd: RawFd },
    Read { fd: RawFd, bgid: u16 },
    Write { fd: RawFd, bgid: u16, bid: usize, len: usize },
    Shutdown { fd: RawFd, bgid: u16 }
}

pub struct Server {
    config: quiche::Config,
    addr: SocketAddr,
    conn_id_seed: ring::hmac::Key,
    conns: HashMap<quiche::ConnectionId<'static>, quiche::Connection>,
    buf_pool: Vec<u16>,
    buf_alloc: Slab<Box<[u8]>>,
    token_alloc: Slab<Token>,
}

impl Server {

    pub fn new(args: ServerArgs) -> Result<Server> {
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        config.load_cert_chain_from_pem_file(&args.cert)?;
        config.load_priv_key_from_pem_file(&args.key)?;
        config.set_application_protos(&[b"perf"])?; 
        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(1_000_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000_000);
        config.set_initial_max_stream_data_uni(1_000_000_000);
        config.set_initial_max_streams_bidi(1);
        config.set_disable_active_migration(true);
        config.enable_early_data();

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();

        Ok(Server {
            config,
            addr: SocketAddr::from_str("127.0.0.1:3456")?,
            conn_id_seed,
            conns: HashMap::new(),
            buf_pool: Vec::with_capacity(64),
            buf_alloc: Slab::with_capacity(1000),
            token_alloc: Slab::with_capacity(1000)
        })
    }

    pub fn listen(&mut self) -> Result<()> {
        let mut ring = IoUring::new(1024)?;
        let (submitter, mut sq, mut cq) = ring.split();
        let listener = UdpSocket::bind(("127.0.0.1", 3456))?;
        let accept = Token::Accept { fd: listener.as_raw_fd() };
    
        let mut backlog = VecDeque::new();

        unsafe {
            sq.push(&self.sqe_from_token(accept))?;
        }

        info!("listening on {:?}", listener.local_addr());

        loop {
            sq.sync();

            match submitter.submit_and_wait(1) {
                Ok(_) => (),
                Err(ref err) if err.raw_os_error() == Some(libc::EBUSY) => (),
                Err(err) => return Err(err.into()),
            }
            cq.sync();

            // clean backlog
            loop {
                if sq.is_full() {
                    match submitter.submit() {
                        Ok(_) => (),
                        Err(ref err) if err.raw_os_error() == Some(libc::EBUSY) => break,
                        Err(err) => return Err(err.into()),
                    }
                }
                sq.sync();

                match backlog.pop_front() {
                    Some(sqe) => unsafe {
                        sq.push(&sqe)?;
                    },
                    None => break,
                }
            }
    
            for cqe in &mut cq {
                let next_tokens = self.handle_cqe(cqe);
    
                if !next_tokens.is_empty() {
                    for tok in next_tokens {    
                        let sqe = self.sqe_from_token(tok);
                        unsafe {
                            if sq.push(&sqe).is_err() {
                                backlog.push_back(sqe);
                            }
                        }
                    }
                }
            }
        }
    }

    fn get_buf_ptr(&mut self, bgid: u16, bid: usize) -> *mut u8 {
        self.buf_alloc[bgid as usize][bid * BUF_LEN..].as_mut_ptr()
    }

    fn get_vacant_bgid(&mut self) -> u16 {
        self.buf_pool.pop().unwrap_or_else(|| {
            let buf = vec![0u8; BUF_NUM * BUF_LEN].into_boxed_slice();
            let buf_entry = self.buf_alloc.vacant_entry();
            let bgid = buf_entry.key();
            buf_entry.insert(buf);
            bgid as u16
        })
    }

    fn handle_cqe(&mut self, cqe: cqueue::Entry) -> Vec<Token> {
        let token_idx = cqe.user_data() as usize;
        let token = self.token_alloc[token_idx].clone();

        let ret = cqe.result();
        if ret < 0 {
            error!("token: {:?} error: {:?}", token, std::io::Error::from_raw_os_error(-ret));
            return vec![];
        }
    
        match token {
            Token::Accept { fd } => self.handle_accept(ret),
            Token::Read { fd, bgid } => self.handle_read(cqe, fd, bgid),
            Token::Write { bgid, bid, .. } => vec![Token::ProvideBuffers { fd: None, addr: self.get_buf_ptr(bgid, bid), num_bufs: 1, buf_len: BUF_LEN as i32, bgid, bid: bid as u16 }],
            Token::Shutdown { fd, bgid } => {
                self.buf_pool.push(bgid);

                vec![]
            },
            Token::ProvideBuffers { fd: Some(fd), bgid, .. } => vec![Token::Read { fd, bgid }],
            _ => vec![]
        }
    }

    fn handle_accept(&mut self, fd: RawFd) -> Vec<Token> {
        debug!("accept {:?}", fd);

        let bgid = self.get_vacant_bgid();
        let addr = self.get_buf_ptr(bgid, 0);
        let read_bufs = Token::ProvideBuffers { fd: Some(fd), addr, num_bufs: BUF_NUM as u16, buf_len: BUF_LEN as i32, bgid, bid: 0 };

        vec![read_bufs]
    }

    fn handle_read(&mut self, cqe: cqueue::Entry, fd: RawFd, bgid: u16) -> Vec<Token> {
        if cqe.result() == 0 && !cqueue::more(cqe.flags()) {
            // here we should cancel reading the sockets
            return vec![Token::Shutdown { fd, bgid }];
        }

        let len = cqe.result() as usize;
        let bid = cqueue::buffer_select(cqe.flags()).unwrap() as usize;
        let mut buf = &mut self.buf_alloc[bgid as usize][bid * BUF_LEN..][..len];

        // Parse the QUIC packet's header.
        let hdr = match quiche::Header::from_slice(buf, quiche::MAX_CONN_ID_LEN) {
            Ok(v) => v,
            Err(e) => {
                error!("Parsing packet header failed: {:?}", e);
                return vec![];
            }
        };

        debug!("got packet {:?}", hdr);

        let conn_id = ring::hmac::sign(&self.conn_id_seed, &hdr.dcid);
        let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
        let conn_id = conn_id.to_vec().into();
        let from = unsafe {
            UdpSocket::from_raw_fd(fd).local_addr().unwrap()
        };

        let conn = if !self.conns.contains_key(&hdr.dcid) && !self.conns.contains_key(&conn_id) {
            if hdr.ty != quiche::Type::Initial {
                error!("Packet is not Initial");
                return vec![];
            }

            if !quiche::version_is_supported(hdr.version) {
                info!("Doing version negotiation");

                let len = quiche::negotiate_version(&hdr.scid, &hdr.dcid, buf).unwrap();
                return vec![Token::Write { fd, bgid, bid, len }];
            }

            let mut scid = [0; quiche::MAX_CONN_ID_LEN];
            scid.copy_from_slice(&conn_id);

            let scid = quiche::ConnectionId::from_ref(&scid);

            // Token is always present in Initial packets.
            let token = hdr.token.as_ref().unwrap();

            // Do stateless retry if the client didn't send a token.
            if token.is_empty() {
                info!("Doing stateless retry");

                let new_token = mint_token(&hdr, &from);

                let len = quiche::retry(
                    &hdr.scid,
                    &hdr.dcid,
                    &scid,
                    &new_token,
                    hdr.version,
                    buf,
                )
                .unwrap();

                return vec![Token::Write { fd, bgid, bid, len }];
            }

            let odcid = validate_token(&from, token);

            // The token was not valid, meaning the retry failed, so
            // drop the packet.
            if odcid.is_none() {
                error!("Invalid address validation token");
                return vec![];
            }

            if scid.len() != hdr.dcid.len() {
                error!("Invalid destination connection ID");
                return vec![];
            }

            // Reuse the source connection ID we sent in the Retry packet,
            // instead of changing it again.
            let scid = hdr.dcid.clone();

            info!("New connection: dcid={:?} scid={:?}", hdr.dcid, scid);

            let conn = quiche::accept(
                &scid,
                odcid.as_ref(),
                self.addr,
                from,
                &mut self.config,
            )
            .unwrap();

            self.conns.insert(scid.clone(), conn);
            self.conns.get_mut(&scid).unwrap()
        } 
        else {
            match self.conns.get_mut(&hdr.dcid) {
                Some(v) => v,
                None => self.conns.get_mut(&conn_id).unwrap(),
            }
        };

        let recv_info = quiche::RecvInfo {
            to: self.addr,
            from,
        };

        // Process potentially coalesced packets.
        let read = match conn.recv(buf, recv_info) {
            Ok(v) => v,
            Err(e) => {
                error!("{} recv failed: {:?}", conn.trace_id(), e);
                return vec![];
            },
        };

        info!("{} processed {} bytes", conn.trace_id(), read);

        while let Ok((read, fin)) = conn.stream_recv(0, buf) {
            if fin {
                let req = &buf[..read];
                info!("conn {:?} received req: {:?}", conn.trace_id(), req);

            }
        }

        // echo
        for stream_id in conn.writable() {
            let sent = match conn.stream_send(stream_id, &buf, true) {
                Ok(cnt) => {
                    debug!("sent some data: {cnt}/{}", buf.len());
                    Ok(cnt)
                },
                Err(quiche::Error::Done) => Ok(0),
                Err(e) => Err(e),
            };

            match sent {
                Ok(len) => {
                    match conn.send(&mut buf[..len]) {
                        Ok(v) => return vec![Token::Write { fd, bgid, bid, len }],
                        Err(quiche::Error::Done) => {
                            return vec![];
                        },
                        Err(e) => {
                            error!("{} send failed: {:?}", conn.trace_id(), e);
                            conn.close(false, 0x1, b"fail").ok();
                            break;
                        },
                    };
                },
                Err(e) => {
                    error!("{} send failed: {:?}", conn.trace_id(), e);
                    conn.close(false, 0x1, b"fail").ok();
                },
            }
        }

        vec![]
    }

    fn sqe_from_token(&mut self, token: Token) -> squeue::Entry {
        let token_idx = self.token_alloc.insert(token.clone());
        let sqe = match token {
            Token::ProvideBuffers { addr, num_bufs, buf_len, bgid, bid, .. } => opcode::ProvideBuffers::new(addr, buf_len, num_bufs, bgid, bid).build(),
            Token::Accept { fd } => opcode::AcceptMulti::new(types::Fd(fd)).build(),
            Token::Read { fd, bgid } => opcode::RecvMulti::new(types::Fd(fd), bgid).build(),
            Token::Write { fd, bgid, bid, len } => opcode::Send::new(types::Fd(fd), self.get_buf_ptr(bgid, bid), len as _).build(),
            Token::Shutdown { fd, .. } => opcode::Shutdown::new(types::Fd(fd), 2).build(),
        };
    
        sqe.user_data(token_idx as _)
    }

}

fn send_blob(conn: &mut Connection, stream_id: u64, blob: &mut Blob) -> Result<()> {
    let buf = vec![0; (blob.size - blob.cursor) as usize];
    let sent = match conn.stream_send(stream_id, &buf, true) {
        Ok(cnt) => {
            debug!("sent some data: {cnt}/{}", buf.len());
            Ok(cnt)
        },
        Err(quiche::Error::Done) => Ok(0),
        Err(e) => Err(e),
    }?;

    blob.cursor += sent as u64;

    Ok(())
}

/// Generate a stateless retry token.
///
/// The token includes the static string `"quiche"` followed by the IP address
/// of the client and by the original destination connection ID generated by the
/// client.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn mint_token(hdr: &quiche::Header, src: &SocketAddr) -> Vec<u8> {
    let mut token = Vec::new();

    token.extend_from_slice(b"quiche");

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    token.extend_from_slice(&addr);
    token.extend_from_slice(&hdr.dcid);

    token
}

/// Validates a stateless retry token.
///
/// This checks that the ticket includes the `"quiche"` static string, and that
/// the client IP address matches the address stored in the ticket.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn validate_token<'a>(
    src: &SocketAddr, token: &'a [u8],
) -> Option<quiche::ConnectionId<'a>> {
    if token.len() < 6 {
        return None;
    }

    if &token[..6] != b"quiche" {
        return None;
    }

    let token = &token[6..];

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    if token.len() < addr.len() || &token[..addr.len()] != addr.as_slice() {
        return None;
    }

    Some(quiche::ConnectionId::from_ref(&token[addr.len()..]))
}


fn main() {
    env_logger::init();

    let args = ServerArgs::parse();
    let code = {
        let res = Server::new(args)
            .and_then(|mut server| server.listen());

        if let Err(e) = res {
            error!("ERROR: {e}");
            1
        } else {
            0
        }
    };
    std::process::exit(code);
}