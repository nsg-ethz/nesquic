use std::{
    net::ToSocketAddrs,
    time::Duration
};
use common::{
    args::ClientArgs,
    perf::{
        Stats,
        create_req, 
        parse_blob_size
    }
};

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use log::{
    info,
    debug,
    error
};
use ring::rand::*;

const MAX_DATAGRAM_SIZE: usize = 1350;

fn run(args: &ClientArgs, stats: &mut Stats) -> Result<()> {
    let remote = (args.url.host_str().unwrap(), args.url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;

    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    config.set_application_protos(&[b"perf"])?;
    config.load_verify_locations_from_file(args.cert.as_str())?;
    config.set_max_idle_timeout(5000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(1_000_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000_000);
    config.set_initial_max_stream_data_uni(1_000_000_000);
    config.set_initial_max_streams_bidi(1);
    config.set_disable_active_migration(true);
    config.verify_peer(false);
    config.enable_early_data();

    // TODO: check what greasing means
    config.grease(false);

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

        error!("send() failed: {e:?}");
    }

    let mut buf = [0; 65535];
    let request = create_req(&args.blob)?;
    let mut req_sent = false;
    let mut res_recv = false;
    let mut res_cnt = 0;

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
            while let Ok((read, fin)) = conn.stream_recv(0, &mut buf) {
                if req_sent {
                    res_cnt += read as u64;
                }

                if req_sent && fin {
                    res_recv = true;
                }
            }
        }
        debug!("done reading");

        // send our perf request
        if conn.is_established() && !req_sent {
            info!("sending perf request {:?}", request);

            conn.stream_send(0, &request, true)?;
            req_sent = true;
            stats.start_measurement();
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

        // we can terminate if the connection is closed
        if conn.is_closed() {
            info!("connection closed, {:?}", conn.stats());

            let (duration, throughput) = stats.stop_measurement()?;
            info!("response received in {:?} - {:.2} Mbit/s", duration, throughput);

            let res_size = parse_blob_size(&args.blob)?;
            if res_size != res_cnt {
                bail!("received blob size ({}B) different from requested blob size ({}B)", res_cnt, res_size)
            }

            break Ok(());
        }

        // we have received our response, we can start closing the connection
        if res_recv && conn.stream_finished(0) && !conn.is_closed() && !conn.is_draining() {
            debug!("closing connection");
            conn.close(true, 0_u32.into(), b"done")?;
        }
    }
}

fn main() {
    env_logger::init();
    
    let args = ClientArgs::parse();
    let blob_size = parse_blob_size(&args.blob).expect("didn't recognize blob size");
    let mut stats = Stats::new(blob_size);
    let mut code = 0;

    for _ in 0..args.reps {
        if let Err(e) = run(&args, &mut stats) {
            error!("ERROR: {e}");
            code = 1;
            break;
        }
    }

    println!("{}", stats.summary());
    std::process::exit(code);
}
