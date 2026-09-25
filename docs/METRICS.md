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
| `nesquic_io`   | `count`, `volume_kb_sum` | `syscall` | Calls of `write`, `writev`, `send`, `sendto`, `sendmsg`, `sendmmsg`, `read`, `readv`, `recv`, `recvfrom`, `recvmsg`, `recvmmsg` on UDP sockets, and the bytes they actually transferred (kB). Failed calls (e.g. `EAGAIN`) count with 0 bytes. | all |
| `nesquic_quic` | `packets_sent`, `packets_received`, `acks_sent`, `acks_received` | | Packets sealed/opened via the BoringSSL AEAD API (`EVP_AEAD_CTX_seal`, `EVP_AEAD_CTX_seal_scatter`, `EVP_AEAD_CTX_open`) and the ACK frames in their payloads. | quinn, quiche, ngtcp2, lsquic, xquic |

The I/O hooks interpose on libc, so libraries that issue raw syscalls or use
io_uring are not covered. The QUIC counters need a dynamically linked,
BoringSSL-compatible libcrypto: noq (ring), neqo (NSS), msquic (statically
linked OpenSSL), picoquic (picotls' own AEAD API) and mvfst
(fizz on OpenSSL's EVP_CIPHER API) report I/O only.

## Debugging

`NQ_QLOG=<path>` additionally writes a qlog trace of all observed packet
headers. This takes a global lock per packet and slows the library down, so
it is off by default (`NQ_QLOG=1 script/run.sh` enables it for servers).
