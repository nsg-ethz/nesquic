use anyhow::{anyhow, bail, ensure, Result};
use glob::glob;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tracing::debug;

#[cfg(feature = "neqo")]
use neqo_iut::qlog_neqo as qlog_iut;
#[cfg(feature = "noq")]
use noq_iut::qlog_noq as qlog_iut;
#[cfg(feature = "quiche")]
use quiche_iut::qlog_quiche as qlog_iut;
#[cfg(feature = "quinn")]
use quinn_iut::qlog_quinn as qlog_iut;

use qlog_iut::events::quic::PacketType;
use qlog_iut::events::EventData;
use qlog_iut::reader::Event;

#[derive(Debug, Clone, Copy, PartialEq)]
struct QlogPacket {
    packet_number: u64,
    time: f32,
}

impl QlogPacket {
    fn new(packet_number: u64, time: f32) -> Self {
        Self {
            packet_number,
            time,
        }
    }
}

/// assumption: initial > handshake > "handshake done" > 1rtt from client
/// to server > 1rtt from server to client.
/// latency = time(1rtt received by client) - time(initial sent by client)
///
/// Pass 1: client: find
/// - client_first_initial_pkt_sent (easy)
/// - client_last_handshake_pkt_sent (just take the last one we see)
/// Pass 2: server: find
/// - server_last_handshake_pkt_received (same pkt_id as client_last_handshake_pkt_sent)
/// - server_first_1rtt_pkt_sent (next packet with type 1rtt)
/// Pass 3: client: find
/// - client_first_1rtt_pkt_received (same pkt_id as server_first_1rtt_pkt_sent)
///
/// Better solution: read from back
#[derive(Debug, Clone, Copy, PartialEq)]
struct QlogResults {
    /// startpoint
    client_first_initial_pkt_sent: Option<QlogPacket>,
    /// find handshake "done" packet by client and server - this should be the
    /// same packet just viewed from different endpoints
    client_last_handshake_pkt_sent: Option<QlogPacket>,
    server_last_handshake_pkt_received: Option<QlogPacket>,
    /// this is (hopefully) the first 1rtt packet sent by server
    /// and received by client that contains actual data
    server_first_1rtt_pkt_sent: Option<QlogPacket>,
    /// endpoint
    client_first_1rtt_pkt_received: Option<QlogPacket>,
}

fn find_sqlog_file(dir: &Path, role: &str) -> Result<PathBuf> {
    let pattern = format!("{}/{}/*qlog", dir.display(), role);
    let file = glob(&pattern)?
        .next()
        .ok_or_else(|| anyhow!("no {} sqlog file in {}", role, dir.display()))??;
    if !file.is_file() {
        bail!("expected a file at {}, got something else", file.display());
    }
    Ok(file)
}

fn open_qlog_file(path: &Path) -> Result<qlog_iut::reader::QlogSeqReader<'_>> {
    qlog_iut::reader::QlogSeqReader::new(Box::new(BufReader::new(File::open(path)?)))
        .map_err(|e| anyhow!("{}", e))
}

/// Three-pass algorithm: extracts handshake latency from client + server qlog files.
///
/// Latency = time(client receives first 1-RTT from server) − time(client sends first Initial).
fn parse_handshake_latency(client_path: &Path, server_path: &Path) -> Result<f32> {
    // Pass 1: client — first Initial sent, last Handshake sent

    let mut results = QlogResults {
        client_first_initial_pkt_sent: None,
        client_last_handshake_pkt_sent: None,
        server_last_handshake_pkt_received: None,
        server_first_1rtt_pkt_sent: None,
        client_first_1rtt_pkt_received: None,
    };
    let mut last_ts = 0.0f32;

    let mut cq = open_qlog_file(client_path)?;
    while let Some(event) = cq.next() {
        let Event::Qlog(ref event) = event else {
            bail!("unexpected non-qlog event in client file");
        };
        // TODO: Some qlog-files do not pass this check, for now we only care for situations
        // where we will update the results.
        // ensure!(event.time >= last_ts, "client qlog events are out of order");
        if let EventData::PacketSent(ref pkt) = event.data {
            match pkt.header.packet_type {
                PacketType::Initial => {
                    // TODO: see above
                    ensure!(event.time >= last_ts, "client qlog events are out of order");
                    results
                        .client_first_initial_pkt_sent
                        .get_or_insert_with(|| {
                            QlogPacket::new(pkt.header.packet_number.unwrap(), event.time)
                        });
                }
                PacketType::Handshake => {
                    // TODO: see above
                    ensure!(event.time >= last_ts, "client qlog events are out of order");
                    results.client_last_handshake_pkt_sent = Some(QlogPacket::new(
                        pkt.header.packet_number.unwrap(),
                        event.time,
                    ));
                }
                _ => {}
            }
        }
        last_ts = event.time;
    }

    ensure!(
        results.client_first_initial_pkt_sent.is_some(),
        "no Initial packet sent by client"
    );
    ensure!(
        results.client_last_handshake_pkt_sent.is_some(),
        "no Handshake packet sent by client"
    );

    // Pass 2: server — matching Handshake received, then first 1-RTT sent after it
    let mut hs_received = false;
    let mut last_ts = 0.0f32;

    let mut sq = open_qlog_file(server_path)?;
    while let Some(event) = sq.next() {
        let Event::Qlog(ref event) = event else {
            bail!("unexpected non-qlog event in server file");
        };
        // TODO: see above
        // ensure!(event.time >= last_ts, "server qlog events are out of order");
        match &event.data {
            EventData::PacketReceived(pkt)
                if matches!(pkt.header.packet_type, PacketType::Handshake) =>
            {
                // TODO: see above
                ensure!(event.time >= last_ts, "server qlog events are out of order");
                if pkt.header.packet_number.unwrap()
                    == results
                        .client_last_handshake_pkt_sent
                        .unwrap()
                        .packet_number
                {
                    hs_received = true;
                    results.server_last_handshake_pkt_received = Some(QlogPacket::new(
                        pkt.header.packet_number.unwrap(),
                        event.time,
                    ));
                }
            }
            EventData::PacketSent(pkt)
                if matches!(pkt.header.packet_type, PacketType::OneRtt) && hs_received =>
            {
                // TODO: see above
                ensure!(event.time >= last_ts, "server qlog events are out of order");
                results.server_first_1rtt_pkt_sent.get_or_insert_with(|| {
                    QlogPacket::new(pkt.header.packet_number.unwrap(), event.time)
                });
            }
            _ => {}
        }
        last_ts = event.time;
        if results.server_first_1rtt_pkt_sent.is_some() {
            break;
        }
    }

    ensure!(
        results.server_last_handshake_pkt_received.is_some(),
        "server never received client's last Handshake packet"
    );

    ensure!(
        results.server_first_1rtt_pkt_sent.is_some(),
        "server never sent 1-RTT packet after handshake"
    );

    // Pass 3: client — first 1-RTT received from server
    let mut cq = open_qlog_file(client_path)?;
    while let Some(event) = cq.next() {
        let Event::Qlog(ref event) = event else {
            bail!("unexpected non-qlog event in client file (pass 3)");
        };
        if let EventData::PacketReceived(pkt) = &event.data {
            if matches!(pkt.header.packet_type, PacketType::OneRtt)
                && pkt.header.packet_number.unwrap()
                    == results.server_first_1rtt_pkt_sent.unwrap().packet_number
            {
                results.client_first_1rtt_pkt_received = Some(QlogPacket::new(
                    pkt.header.packet_number.unwrap(),
                    event.time,
                ));
                break;
            }
        }
    }

    ensure!(
        results.client_first_1rtt_pkt_received.is_some(),
        "client never received server's first 1-RTT packet"
    );

    let first_initial_pkt = results
        .client_first_initial_pkt_sent
        .ok_or_else(|| anyhow!("no Initial packet sent by client"))?;
    let first_1rtt_client = results
        .client_first_1rtt_pkt_received
        .ok_or_else(|| anyhow!("client never received server's first 1-RTT packet"))?;

    let latency_ms = first_1rtt_client.time - first_initial_pkt.time;
    debug!("qlog handshake latency: {:.3} ms", latency_ms);
    Ok(latency_ms)
}

/// Parse qlog files from `qlog_dir/{client,server}/*qlog` and return an InfluxDB
/// line-protocol string for the `nesquic_qlog` measurement.
pub(crate) fn extract_metrics(
    qlog_dir: &Path,
    tag_str: &str,
    timestamp_ns: u128,
) -> Result<String> {
    let client_path = find_sqlog_file(qlog_dir, "client")?;
    let server_path = find_sqlog_file(qlog_dir, "server")?;
    let latency_ms = parse_handshake_latency(&client_path, &server_path)?;
    Ok(format!(
        "nesquic_qlog{} handshake_latency_ms={} {}",
        tag_str, latency_ms, timestamp_ns
    ))
}
