# Nesquic Container CLI

This document specifies the command-line interface that a per-library Docker
container must expose. It is derived from the `nesquic` binary
(`nesquic/src/main.rs`), the shared argument definitions
(`utils/src/bin.rs`), and the way `script/run.sh` invokes the binary.

Only **quinn** is considered here. Each library is built into its own binary
(`script/run.sh` compiles with `--features <lib>` and renames the output to
`nesquic-<lib>`), so the quinn container ships a `nesquic` binary built with the
`quinn` Cargo feature.

## Invocation

```
nesquic <client|server> [COMMON OPTIONS] [SUBCOMMAND OPTIONS] <POSITIONAL>
```

The binary has exactly two subcommands, `client` and `server`, and also
supports the clap-provided `--help` and `--version` flags.

Because the binary is compiled with only the `quinn` feature, `--lib` must be
`quinn`; any other value fails at runtime with *"selected library is not enabled
in this build"*.

## Common options

These apply to both `client` and `server` (flattened from `CommonArgs`):

| Flag | Value | Required | Description |
|------|-------|----------|-------------|
| `-l`, `--lib` | enum | yes | QUIC library to run. Must be `quinn` for this container. (Accepted enum values: `quinn`, `quiche`, `msquic`, `ngtcp`, `neqo`, `noq`.) |
| `-j`, `--job` | string | no | Experiment/job name. When set **and** the `INFLUX_*` env vars are present, metrics are pushed to InfluxDB under this name; otherwise metrics are printed locally. |
| `-L` | `key:value` | no | Run label, `key:value` form, may be repeated. Added to the metric label set (e.g. `-L nesquic_run:firstRun`). |

## `client` subcommand

Common options above, plus (`ClientArgs`):

| Arg | Value | Required | Default | Description |
|-----|-------|----------|---------|-------------|
| `<url>` (positional) | URL | no | `https://127.0.0.1:4433` | Server URL. Host is used for cert validation and resolution; port defaults to `4433` if absent. |
| `-c`, `--cert` | path | yes | — | PEM certificate to trust (the server's cert / CA). Must be readable inside the container. |
| `-b`, `--blob` | string | yes | — | Requested payload size, e.g. `50Mbit`. Format: `<number>[G|M|K]bit` (see `docs/PROTOCOL.md` §3). |
| `--unencrypted` | flag | no | `false` | Present in the shared args but **ignored** by the quinn IUT. |

The client connects, performs one request/response exchange, records the
measurement, and exits.

## `server` subcommand

Common options above, plus (`ServerArgs`):

| Arg | Value | Required | Default | Description |
|-----|-------|----------|---------|-------------|
| `<listen>` (positional) | `addr:port` | no | `0.0.0.0:4433` | Address/port to listen on. |
| `-c`, `--cert` | path | yes* | — | PEM certificate chain. Requires `--key`. |
| `-k`, `--key` | path | yes* | — | PEM private key. Requires `--cert`. |
| `--unencrypted` | flag | no | `false` | Present in the shared args but **ignored** by the quinn IUT. |

\* `--cert` and `--key` mutually require each other; supply both.

The server runs indefinitely, serving connections until it receives `SIGINT` or
`SIGTERM` (see below).

## Environment variables

| Variable | Purpose |
|----------|---------|
| `INFLUX_URL`, `INFLUX_TOKEN`, `INFLUX_ORG`, `INFLUX_BUCKET` | InfluxDB target for metric push. All four must be set (together with `-j/--job`) for metrics to be pushed; otherwise the run reports metrics locally. |
| `RUST_LOG` | Tracing filter (`tracing_subscriber` `EnvFilter`), e.g. `RUST_LOG=quinn::client=trace`. |

## Lifecycle & signals

- The process installs a metrics collector (eBPF I/O monitor) before starting
  the job and pushes/reports metrics after the job completes.
- `SIGINT` (Ctrl-C) and `SIGTERM` cancel the running job; the server relies on
  this for shutdown, after which it flushes metrics.
- Exit status is non-zero if the job returns an error.

## Examples

Mirroring `script/run.sh` (paths assume the cert/key are mounted into the
container):

Server:
```
INFLUX_URL=http://10.0.0.1:8086 INFLUX_TOKEN=nesquic-token \
INFLUX_ORG=nesquic INFLUX_BUCKET=nesquic \
nesquic server -j unbounded --lib quinn \
  --cert /res/pem/cert.pem --key /res/pem/key.pem \
  0.0.0.0:4433 -L nesquic_run:firstRun
```

Client:
```
INFLUX_URL=http://10.0.0.1:8086 INFLUX_TOKEN=nesquic-token \
INFLUX_ORG=nesquic INFLUX_BUCKET=nesquic \
nesquic client -j unbounded --lib quinn \
  --cert /res/pem/cert.pem --blob 50Mbit \
  https://10.0.0.1:4433 -L nesquic_run:firstRun
```
