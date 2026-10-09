# Frontend

`docker compose -f docker/service.yml up -d` (also done by `script/run.sh`)
starts InfluxDB and the frontend at <http://localhost:3000>.

The frontend (`frontend/`, served by nginx, see `docker/Dockerfile.frontend`)
queries InfluxDB directly, so new results show up on refresh; there is no
dashboard to regenerate. Pick a library (or "Overview", the default, to
compare all of them), a run (`nesquic_run` label, or all runs) and a time
range. Every bar is the mean over all matching
measurements; the error bar spans one standard deviation.

The overview shows, per experiment, client throughput and mean request latency of every library. The latency bars are stacked: the crypto and I/O durations per request of client and server together (see [metrics](METRICS.md)), and the rest of the latency ("Other"). A client is paired with the server that reports right after it, as in `script/run.sh`. Panels of a single library:

- **Overview**: client throughput and mean request latency per experiment.
- **Per experiment** (from `res/experiments.yaml`): syscall count and data
  volume per I/O syscall, for server and client; ACK frames and packets sent
  per side (only for libraries with QUIC counters, see [metrics](METRICS.md)).
- **qlog**: if `res/qlog/<library>/<job>.<server|client>.qlog`
  exists, `script/qlog.sh [library...]` (called by `script/run.sh`) renders it
  with [qvis](https://github.com/larseggert/qvis) (needs `uvx`) to a
  self-contained `.html` page next to it, which the frontend embeds below the
  charts.

nginx forwards `POST /api/query` to InfluxDB's `/api/v2/query` with the
token from `INFLUX_TOKEN`; there is no authentication in front of it.

Dependencies (D3, js-yaml, uchū) are managed with [bun](https://bun.sh)
(`frontend/package.json`, `frontend/bun.lock`); the image bundles the page
with `bun run build`. For local work:

```sh
cd frontend && bun install && bun run build   # writes frontend/dist
```

After changing the frontend, rebuild the image:

```sh
docker compose -f docker/service.yml up -d --build frontend
```
