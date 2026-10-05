# Metrics

Metrics are collected by `libnesquic.so` (`nesquic/`), which every IUT image
registers in `/etc/ld.so.preload`. It is active only in `nesquic-*` binaries
(override with `NQ_ENABLE=1`/`0`), aggregates everything in memory, and
reports once, when the process exits:

- if a job is given (`-j`, see [CLI](CLI.md)) and `INFLUX_URL`,
  `INFLUX_TOKEN`, `INFLUX_ORG` and `INFLUX_BUCKET` are set, it writes to
  InfluxDB (`/api/v2/write`, plain HTTP, 1.5 s timeout);
- otherwise it prints the line protocol to stdout.

## Tags

Every point carries `job`, `mode` (`client`/`server`), `library`, `version`
and all `-L key:value` labels. `library` and `version` come from
`NQ_LIBRARY`/`NQ_LIBRARY_VERSION`, which the Rust IUTs set themselves; the
library defaults to the binary name without its `nesquic-` prefix.

## Measurements

| Measurement    | Fields | Tags | Source | Libraries |
|----------------|--------|------|--------|-----------|
| `nesquic`      | `throughput` | | UDP bytes received / time between first and last received datagram, in bytes / 10^6 / s (the unit of `utils::perf::Stats`). Clients only. | all |
| `nesquic_latency` | `ttfb_ms`, `request_latency_ms` | | Clients only, from decrypted packets (see below). `ttfb_ms`: first UDP datagram sent (connection start, so including the handshake) → first received packet with STREAM data. `request_latency_ms`: first sent packet with STREAM data (the request) → last received packet with STREAM data or FIN (the end of the response). | all |
| `nesquic_io`   | `count`, `volume_kb_sum` | `syscall` | Calls of `write`, `writev`, `send`, `sendto`, `sendmsg`, `sendmmsg`, `read`, `readv`, `recv`, `recvfrom`, `recvmsg`, `recvmmsg` on UDP sockets, and the bytes they actually transferred (kB). Failed calls (e.g. `EAGAIN`) count with 0 bytes. | all |
| `nesquic_quic` | `packets_sent`, `packets_received`, `acks_sent`, `acks_received` | | Packets sealed/opened by the library's AEAD (see below) and the ACK frames in their payloads. | all |


`script/run.sh` runs every experiment in three modes:

1. `detached`: without `libnesquic.so` (`/etc/ld.so.preload` is masked);
2. `attached`: `libnesquic.so` reports all measurements above;
3. `qlog`: `libnesquic.so` writes qlog traces (see [Debugging](#debugging))
   and no metrics are uploaded.

`NQ_REPETITIONS` (default 1) repeats the `detached` and `attached` modes that
many times; the dashboard shows the mean of the newest `script/run.sh`
invocation's repetitions.

Independently of `libnesquic.so`, every client prints its own measurement to
stdout, from writing the request to reading the end of the response
(`nq_report` in `iut/c-common`, `run_client` in `iut/common`):

```
nesquic_app throughput=<bytes / 10^6 / s>,request_latency_ms=<ms>
```

It is not uploaded to InfluxDB.

The dashboard derives *Packets Dropped* from these: the server's `packets_sent`
minus the client's `packets_received`. The server also counts packets it sends
after the client is gone.

The I/O hooks interpose on libc, so libraries that issue raw syscalls or use
io_uring are not covered.

The QUIC counters, latencies and qlog come from hooking the AEAD calls that
protect each packet: its associated data is the unprotected header and its
plaintext the frames. This needs the crypto library to be linked dynamically.

| API | Hooked functions | Libraries |
|-----|------------------|-----------|
| BoringSSL / AWS-LC | `EVP_AEAD_CTX_seal`, `EVP_AEAD_CTX_seal_scatter`, `EVP_AEAD_CTX_open` | quinn, noq (AWS-LC), quiche, ngtcp2, lsquic, xquic |
| OpenSSL 3 | `EVP_{En,De}cryptInit_ex`, `EVP_CipherInit_ex`, `EVP_{En,De}cryptUpdate`, `EVP_{En,De}cryptFinal_ex` | msquic, picoquic (picotls), mvfst (fizz) |
| NSS | `SSL_AeadEncrypt`, `SSL_AeadDecrypt` (via `SSL_GetExperimentalAPI`) | neqo |

`script/test.sh` checks for every library that the client reports packet
counts, TTFB and request latency, and that both sides write a qlog trace with sent and received packets.

## Debugging

`NQ_QLOG=<path>` additionally writes a qlog trace (JSON-SEQ) of all observed
packet headers. This takes a global lock per packet and slows the library
down, so it is off by default. `script/run.sh` enables it in a separate run of
every experiment and writes `<job>.client.qlog` and `<job>.server.qlog` to
`res/qlog/<library>/`.
