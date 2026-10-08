# Benchmarking Protocol

This document specifies the application-level protocol that every QUIC
implementation under test (IUT) must speak.

Any new library must provide a **client** (sender of the request / receiver of
the blob) and a **server** (receiver of the request / sender of the blob) that
are wire-compatible with the description below. 

## Roles

The benchmark consists of request/response exchanges, each over its own QUIC
bidirectional stream:

- The **client** connects, opens a bidirectional stream, sends a fixed 8-byte
  **request** stating how large a payload it wants, then reads the response
  until the server closes the stream.
- The **server** listens, accepts connections and streams, reads the 8-byte
  request, and writes back exactly that many bytes (a **blob**) before
  finishing the stream.

The transferred payload is the unit of throughput measurement. By default
there is exactly **one** request/response per measured run; the client's
`--connections`, `--streams` and `--duration` (see [CLI](CLI.md)) run several
concurrently and back to back.

## Transport configuration

All IUTs must configure QUIC/TLS identically so that connections are
interchangeable:

| Parameter           | Value                                                        |
|---------------------|--------------------------------------------------------------|
| TLS version         | 1.3 (QUIC mandates it)                                       |
| ALPN protocol       | `perf` (single entry: the ASCII bytes `b"perf"`)            |
| Client auth         | none (`with_no_client_auth`)                                |
| Server certificate  | single cert + key, PEM files supplied via CLI               |
| Client trust        | root store seeded with the server cert PEM (no system roots)|
| Default port        | `4433`                                                       |
| Server listen addr  | `0.0.0.0:4433` by default                                   |
| Client bind addr    | `[::]:0` (ephemeral, dual-stack)                            |
| Idle timeout        | 10 s                                                         |
| Stream receive window | 8 MiB (`initial_max_stream_data_bidi_*`)                  |
| Connection receive window | 16 MiB (`initial_max_data`)                           |
| Bidirectional streams | 100 (`initial_max_streams_bidi`), the most `--streams` a client accepts; it must wait when the limit is reached |
| Congestion control  | Cubic                                                        |
| UDP socket buffers  | `SO_RCVBUF` = `SO_SNDBUF` = 16 MiB requested; the kernel clamps both to `net.core.{r,w}mem_max` |
| Response payload    | zeros, written in chunks of a static 64 KiB buffer, never materialised as a whole |
| Datagram I/O        | libc syscalls only (see [Metrics](METRICS.md)), batched wherever the library supports it: `UDP_SEGMENT` (GSO), `UDP_GRO`, `sendmmsg`, `recvmmsg` |

The values live in `utils::perf` (Rust IUTs) and `iut/c-common/nesquic.h`
(C/C++ IUTs). What a library cannot be configured to do is listed in
[Libraries](LIBRARIES.md).

The ALPN string **must** be exactly `perf`; a mismatch fails the handshake.

## Request wire format

The request is a fixed **8-byte, big-endian unsigned integer** giving the
desired blob size **in bytes**.

```
+--------+--------+--------+--------+--------+--------+--------+--------+
|                    size in bytes (u64, big-endian)                   |
+--------+--------+--------+--------+--------+--------+--------+--------+
  byte 0                                                        byte 7
```

- Client side: `Request { size }.to_bytes()` == `size.to_be_bytes()` (`usize`,
  i.e. 8 bytes on a 64-bit target).
- Server side: read the first 8 bytes, `usize::from_be_bytes(..)` == blob size
  in bytes. (`Blob::try_from(&[u8])` reads `value[0..8]`.)

The request stream may carry only these 8 bytes; the server reads up to 64 KiB
but only the leading 8 bytes are significant.

## Connection & stream lifecycle

### Client

1. **init**: build the client TLS/QUIC config (root store from the cert PEM,
   ALPN `perf`). No I/O.
2. **connect**: resolve `url` host + port (default 4433), create an endpoint
   bound to `[::]:0`, and establish the QUIC connection using the URL host as
   the server name for certificate validation. Store the connection.
3. **run** (this is the measured section), for every request:
   1. Open a **bidirectional** stream.
   2. Write the 8-byte request, then **finish** the send side
      (signals end of request).
   3. Read the response to end-of-stream.
   4. Assert the received length equals the requested size; mismatch is a
      hard error.

After `run` returns, the client connection is dropped, which closes the QUIC
connection (application close). A new IUT should likewise tear the connection
down after its last exchange so the server's accept loop unblocks (see
below).

### Server

1. **init**: build the server TLS/QUIC config (single cert+key, ALPN `perf`).
2. **listen**: bind the endpoint to the listen address and loop accepting
   incoming connections. Each connection is handled on its own task.
3. **per connection**: loop accepting bidirectional
   streams. Each accepted stream `(send, recv)` is handled on its own task.
   - When the peer closes the connection at the application layer, the loop returns cleanly — this is
     the normal end-of-client signal, **not** an error.
   - Other connection errors propagate as failures.
4. **per request**: read the request (up to 64 KiB) to
   end-of-stream, parse the 8-byte size, write that many bytes, then **finish**
   the send stream.

### Summary of the exchange

```
client                                   server
  |  --- QUIC handshake (ALPN "perf") --->  |
  |  open bidirectional stream              |
  |  --- 8-byte request (size BE) ------->  |
  |  finish send side                       |  read request (<=64 KiB), parse size
  |  <---------- size bytes -------------    |  write blob
  |              (read to end)              |  finish send side
  |  verify len == size                     |
  |  drop connection (application close) -> |  accept_bi -> ApplicationClosed -> done
```
