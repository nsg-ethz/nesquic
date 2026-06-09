# Nesquic Benchmarking Protocol

This document specifies the application-level protocol that every QUIC
implementation under test (IUT) must speak. It is derived from the reference
`quinn` implementation (`iut/quinn/src/lib.rs` +
`iut/common/quinn-noq/{mod,client,server}.rs`) and the shared harness in
`utils/src/{bin,perf}.rs` and `nesquic/src/{lib,main}.rs`.

Any new library must provide a **client** (sender of the request / receiver of
the blob) and a **server** (receiver of the request / sender of the blob) that
are wire-compatible with the description below. As long as the wire format and
connection lifecycle match, an IUT's client can interoperate with any other
IUT's server.

## 1. Roles

The benchmark is a single request/response exchange over one QUIC bidirectional
stream:

- The **client** connects, opens a bidirectional stream, sends a fixed 8-byte
  **request** stating how large a payload it wants, then reads the response
  until the server closes the stream.
- The **server** listens, accepts connections and streams, reads the 8-byte
  request, and writes back exactly that many bytes (a **blob**) before
  finishing the stream.

The transferred payload is the unit of throughput measurement. There is exactly
**one** request/response per measured run (see §6).

## 2. Transport configuration

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

The ALPN string **must** be exactly `perf`; a mismatch fails the handshake.

### Sockets

The reference binds a raw UDP socket via `socket2` (`bind_socket`):

- `Domain::for_address(addr)`, `Type::DGRAM`, `Protocol::UDP`.
- For IPv6 addresses, dual-stack is enabled with `set_only_v6(false)` so the
  socket accepts both IPv4 and IPv6.
- The client binds `[::]:0`; the server binds its configured listen address.

A new IUT may bind the socket however its API requires, but should preserve the
dual-stack behaviour so the same address conventions work.

### Certificates

- Certificate and private key are passed as PEM file paths on the command line
  (`--cert`, `--key`).
- The repository ships test material at `res/pem/cert.pem` and `res/pem/key.pem`
  (see `utils/src/bin::{ClientArgs,ServerArgs}::test`).
- The `--unencrypted` flag exists in the shared CLI args ("do TLS handshake but
  don't encrypt") but is **not** honoured by the reference quinn IUT. Treat it
  as optional; encrypted operation is the baseline.

## 3. Request wire format

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

### Deriving the size from the CLI blob string

The client receives the desired size as a human string via `--blob` (e.g.
`50Mbit`). Parsing rules (`Request::try_from(String)`):

- The string must be at least 4 chars long and **end with `bit`**.
- The character immediately before `bit` may be a unit prefix:
  - `G` → ×1 000 000 000
  - `M` → ×1 000 000
  - `K` → ×1 000
  - a digit → no prefix (the leading number is taken as-is)
  - any other letter → error.
- The leading numeric portion is parsed as an integer count of **bits**.
- The byte size sent on the wire is `bits / 8` (integer division).

Examples:
- `100Mbit` → 100 000 000 bits → **12 500 000 bytes** on the wire.
- `100bit`  → 100 bits → **12 bytes** on the wire.
- `20Gbit`  → 20 000 000 000 bits → 2 500 000 000 bytes.

A new IUT's client only needs to send the resulting byte count as the 8-byte
request; the harness performs the string parsing (`run_client` parses the blob
string itself to know the expected size).

## 4. Blob (response) format

The server responds with exactly `size` bytes, where `size` is the value from
the request. The reference fills the payload with **zero bytes** (`Blob` is an
iterator that yields `0u8` `size` times). Payload content is not validated by
the client beyond its length, so any bytes are acceptable, but zeros match the
reference.

The server writes the blob and then **finishes** (cleanly closes the send half
of) the stream. The end-of-stream signal is how the client knows the transfer
is complete.

## 5. Connection & stream lifecycle

### Client (`bin::Client`)

The harness drives the client through three trait methods
(`utils/src/bin.rs`): `new(args)`, then `connect()`, then `run()`.

1. **new**: build the client TLS/QUIC config (root store from the cert PEM,
   ALPN `perf`). No I/O.
2. **connect**: resolve `url` host + port (default 4433), create an endpoint
   bound to `[::]:0`, and establish the QUIC connection using the URL host as
   the server name for certificate validation. Store the connection.
3. **run** (this is the measured section):
   1. Open a **bidirectional** stream.
   2. Write the 8-byte request, then **finish** the send side
      (signals end of request).
   3. Read the response to end-of-stream (`read_to_end`).
   4. Assert the received length equals the requested size; mismatch is a
      hard error.

After `run` returns, the client connection is dropped, which closes the QUIC
connection (application close). A new IUT should likewise tear the connection
down after the single exchange so the server's accept loop unblocks (see
below).

### Server (`bin::Server`)

The harness calls `new(args)` then `listen()`, which runs until terminated by a
signal.

1. **new**: build the server TLS/QUIC config (single cert+key, ALPN `perf`).
2. **listen**: bind the endpoint to the listen address and loop accepting
   incoming connections. Each connection is handled on its own task.
3. **per connection** (`handle_connection`): loop accepting bidirectional
   streams. Each accepted stream `(send, recv)` is handled on its own task.
   - When the peer closes the connection at the application layer
     (`ConnectionError::ApplicationClosed`), the loop returns cleanly — this is
     the normal end-of-client signal, **not** an error.
   - Other connection errors propagate as failures.
4. **per request** (`handle_request`): read the request (up to 64 KiB) to
   end-of-stream, parse the 8-byte size, write that many bytes, then **finish**
   the send stream.

The server is concurrent and long-lived: it serves many connections/streams and
is expected to keep running across repeated client invocations. It is stopped
externally via Ctrl-C / SIGTERM (`select_with_term_signals` in
`nesquic/src/main.rs`).

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

## 6. Measurement & metrics

Timing is handled entirely by the harness (`utils/src/perf.rs::Stats`,
`nesquic/src/lib.rs::run_client`); an IUT does not record timing itself.

- The harness parses `--blob` to learn the expected byte count, builds the
  client, calls `connect()` (outside the timer), then:
  1. `start_measurement()` — records `Instant::now()`.
  2. `client.run()` — the full request + blob transfer.
  3. `add_bytes(size)` — records the transferred byte count.
  4. `stop_measurement()` — records the elapsed duration.
- **One measured run = one request/response.** The reference performs a single
  rep per `run_client` invocation; repetition across runs is orchestrated
  externally (the benchmark scripts), with each run pushing its mean throughput
  into `THROUGHPUT_SAMPLES`.
- Throughput is computed as `bytes / 1_000_000 / seconds`
  (`Stats::calculate_throughput`). Because the request size is expressed in
  bytes, this is megabytes-per-second numerically even though the summary string
  labels it `Mbit/s`; the same formula is applied uniformly to all IUTs, so
  cross-library comparisons remain valid.
- `Stats::summary()` reports rep count, mean ± stderr duration, and mean ±
  stderr throughput.

Library-internal metrics (I/O, crypto, etc.) are gathered out-of-band via eBPF
by the `MetricsCollector` and are independent of the wire protocol; an IUT does
not need to emit them.

## 7. Implementation checklist for a new library

To add an IUT, implement `utils::bin::Client` and `utils::bin::Server` for the
new library such that:

- [ ] TLS 1.3 with ALPN exactly `perf`; no client auth.
- [ ] Server loads single cert+key from the `--cert`/`--key` PEM paths; client
      trusts only the server cert PEM and validates against the URL host.
- [ ] Client binds `[::]:0` (dual-stack), connects to host:port (default 4433).
- [ ] Server binds the `--listen` address and serves connections/streams
      concurrently and indefinitely.
- [ ] Client opens **one bidirectional stream**, writes the 8-byte big-endian
      byte-count request, finishes its send side, reads the response to
      end-of-stream, and verifies the length.
- [ ] Server reads the 8-byte request, writes exactly that many bytes, and
      finishes the stream.
- [ ] Server treats application-level connection close as a clean shutdown of
      that connection, not an error.
- [ ] Wire the new library into `nesquic/src/lib.rs` (`Library` enum +
      feature-gated `run_client`/`run_server` arms) behind its own Cargo
      feature, mutually exclusive with the other IUTs.

## 8. Known deviations

The current IUTs follow the wire format (§3–§4) and ALPN, but a few do not yet
match the TLS-trust and verification details above:

- **quiche**: the client does not load the server CA cert (relies on
  tokio-quiche defaults) and does not verify the received blob length.
- **neqo**: ignores the `--cert`/`--key` PEM arguments — the server uses an NSS
  DB nickname and the client accepts the server certificate unconditionally
  (trust comes from the NSS DB). It also enables 0-RTT/session tickets.
- **msquic**: work in progress, not enabled in the build; hardcodes port 4433
  on the client and uses an idle-timeout-based server accept loop.

`quinn` and `noq` are fully conformant (`noq` reuses the same reference code).
