use std::{
    net::ToSocketAddrs,
    time::{Duration, Instant}
};

use anyhow::{anyhow, Result};
use clap::Parser;
use log::{
    info,
    debug,
    error
};
use ring::rand::*;
use url::Url;

const MAX_DATAGRAM_SIZE: usize = 1350;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {    
    url: Url,
    /// do TLS handshake, but don't encrypt connection
    #[clap(long = "unencrypted")]
    unencrypted: bool,
    /// TLS certificate in PEM format
    #[clap(short = 'c', long = "cert")]
    cert: String,
}

fn run(args: Args) -> Result<()> {
    let remote = (args.url.host_str().unwrap(), args.url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    config.set_application_protos(&[b"perf"])?;
    config.load_verify_locations_from_file(args.cert.as_str())?;
    config.verify_peer(false);
    config.set_max_idle_timeout(5000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_stream_data_uni(1_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_disable_active_migration(true);
    config.enable_early_data();

    // TODO: check what greasing means
    // config.grease(false);

    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    let rng = SystemRandom::new();
    rng.fill(&mut scid[..]).unwrap();
    let scid = quiche::ConnectionId::from_ref(&scid);

    let bind_addr = match remote {
        std::net::SocketAddr::V4(_) => "0.0.0.0:0",
        std::net::SocketAddr::V6(_) => "[::]:0",
    };

    let mut socket = mio::net::UdpSocket::bind(bind_addr.parse()?)?;

    let mut poll = mio::Poll::new()?;
    let mut events = mio::Events::with_capacity(1024);
    poll.registry()
        .register(&mut socket, mio::Token(0), mio::Interest::READABLE)?;

    let host = args.url
    .host_str()
    .ok_or_else(|| anyhow!("no hostname specified"))?;
    info!("connecting to {host} at {remote}");

    let local_addr = socket.local_addr().unwrap();
    let mut conn = quiche::connect(args.url.domain(), &scid, local_addr, remote, &mut config)?;

    let mut out = [0; MAX_DATAGRAM_SIZE];
    let (write, send_info) = conn.send(&mut out).expect("initial send failed");

    while let Err(e) = socket.send_to(&out[..write], send_info.to) {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            info!(
                "{} -> {}: send() would block",
                socket.local_addr().unwrap(),
                send_info.to
            );
            continue;
        }

        return Err(anyhow!(format!("send() failed: {e:?}")));
    }

    let mut buf = [0; 65535];
    let mut pkt_count = 0;
    let start = Instant::now();

    // Prepare request.
    let request = format!("GET {}\r\n", args.url.path());
    let mut req_sent = false;
    let mut res_rcvd = false;

    loop {
        poll.poll(&mut events, conn.timeout()).unwrap();

        // Read incoming UDP packets from the socket and feed them to quiche,
        // until there are no more packets to read.
        'read: loop {
            // If the event loop reported no events, it means that the timeout
            // has expired, so handle it without attempting to read packets. We
            // will then proceed with the send loop.
            if events.is_empty() {
                debug!("timed out");

                conn.on_timeout();

                break 'read;
            }

            let (len, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,

                Err(e) => {
                    // There are no more UDP packets to read, so end the read
                    // loop.
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("recv() would block");
                        break 'read;
                    }

                    panic!("recv() failed: {:?}", e);
                },
            };

            debug!("got {} bytes", len);

            let recv_info = quiche::RecvInfo {
                to: local_addr,
                from,
            };

            // Process potentially coalesced packets.
            let read = match conn.recv(&mut buf[..len], recv_info) {
                Ok(v) => v,

                Err(e) => {
                    error!("recv failed: {:?}", e);
                    continue 'read;
                },
            };

            debug!("processed {} bytes", read);
        }

        debug!("done reading");

        if conn.is_closed() {
            info!("connection closed, {:?}", conn.stats());
            break Ok(());
        }

        // we have received our response, we can close the connection
        if conn.is_established() && req_sent && conn.stream_finished(0_u32.into()) {
            debug!("closing connection");
            conn.close(false, 0_u32.into(), b"done")?;
        }

        // send our perf request
        if conn.is_established() && !req_sent {
            info!("sending perf request {:?}", request);

            let buf = request.as_bytes();
            conn.stream_send(0, buf, true)?;
            req_sent = true;
        }

        // Generate outgoing QUIC packets and send them on the UDP socket, until
        // quiche reports that there are no more packets to be sent.
        loop {
            let (write, send_info) = match conn.send(&mut out) {
                Ok(v) => v,

                Err(quiche::Error::Done) => {
                    debug!("done writing");
                    break;
                },

                Err(e) => {
                    error!("send failed: {:?}", e);

                    conn.close(false, 0x1, b"fail").ok();
                    break;
                },
            };

            if let Err(e) = socket.send_to(&out[..write], send_info.to) {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    debug!("send() would block");
                    break;
                }

                panic!("send() failed: {:?}", e);
            }

            debug!("written {}", write);
        }

        if conn.is_closed() {
            info!("connection closed, {:?}", conn.stats());
            break Ok(());
        }
    }
}

fn main() {
    env_logger::init();

    let args = Args::parse();
    let code = {
        if let Err(e) = run(args) {
            error!("ERROR: {e}");
            1
        } else {
            0
        }
    };
    std::process::exit(code);
}
