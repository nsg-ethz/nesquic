# Container CLI

This document specifies the command-line interface that a per-library Docker
container must expose. 

## Invocation

```
[BINARY] <client|server> [COMMON OPTIONS] [SUBCOMMAND OPTIONS] <POSITIONAL>
```

The binary has two subcommands, `client` and `server`.

## Common options

These apply to both `client` and `server` (flattened from `CommonArgs`):

| Flag | Value | Required | Description |
|------|-------|----------|-------------|
| `-j`, `--job` | string | no | Experiment/job name. When set **and** the `INFLUX_*` env vars are present, metrics are pushed to InfluxDB under this name; otherwise metrics are printed locally. |
| `-L` | `key:value` | no | Run label, `key:value` form, may be repeated. Added to the metric label set (e.g. `-L nesquic_run:firstRun`). |

## `client` subcommand

Common options above, plus (`ClientArgs`):

| Arg | Value | Required | Default | Description |
|-----|-------|----------|---------|-------------|
| `<url>` (positional) | URL | no | `https://127.0.0.1:4433` | Server URL. Host is used for cert validation and resolution; port defaults to `4433` if absent. |
| `-c`, `--cert` | path | yes | — | PEM certificate to trust (the server's cert / CA). |
| `-b`, `--blob` | string | yes | — | Requested payload size, e.g. `50Mbit`. Format: `<number>[G|M|K]bit` (see `docs/PROTOCOL.md` §3). |
| `--unencrypted` | flag | no | `false` | Leaves traffic unencrypted if set. |

The client connects, performs one request/response exchange, records the measurement, and exits.

### Deriving the size from the CLI blob string

The client receives the desired size as a human string via `--blob` (e.g.
`50Mbit`):

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

## `server` subcommand

Common options above, plus (`ServerArgs`):

| Arg | Value | Required | Default | Description |
|-----|-------|----------|---------|-------------|
| `<listen>` (positional) | `addr:port` | no | `0.0.0.0:4433` | Address/port to listen on. |
| `-c`, `--cert` | path | yes | — | PEM certificate chain. Requires `--key`. |
| `-k`, `--key` | path | yes | — | PEM private key. Requires `--cert`. |
| `--unencrypted` | flag | no | `false` | Leaves traffic unencrypted if set. |

The server runs indefinitely, serving connections until it receives `SIGINT` or `SIGTERM` (see below).

## Crypto library linkage

Every per-library binary must link its crypto library (e.g. quiche's BoringSSL)
**dynamically**, so that nesquic's `LD_PRELOAD` monitor (`libnesquic.so`)
can interpose the crypto functions at runtime. A statically linked crypto
library binds its symbols internally and cannot be intercepted.

## Lifecycle & signals

- `SIGINT` (Ctrl-C) and `SIGTERM` cancel the running job; the server relies on
  this for shutdown, after which it flushes metrics.
- Exit status is non-zero if the job returns an error.
