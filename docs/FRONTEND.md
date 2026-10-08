# Frontend

`docker compose -f docker/service.yml up -d` (also done by `script/run.sh`)
starts InfluxDB and the frontend at <http://localhost:3000>.

The frontend (`frontend/`, served by nginx, see `docker/Dockerfile.frontend`)
queries InfluxDB directly, so new results show up on refresh; there is no
dashboard to regenerate. Pick a library (or "Overview", the default, to
compare all of them), a run (`nesquic_run` label, or all runs) and a time
range. Every bar is the mean over all matching
measurements; the error bar spans one standard deviation.

The overview shows, per experiment, client throughput of every library. Panels of a single library:

- **Overview**: client throughput per experiment.
- **Per experiment** (from `res/experiments.yaml`): syscall count and data
  volume per I/O syscall, for server and client; ACK frames and packets sent
  per side (only for libraries with QUIC counters, see [metrics](METRICS.md)).
- **qlog**: if `res/qlog/<library>/<job>.qlog` exists (`NQ_QLOG=1
  script/run.sh`), it can be opened in [qvis](https://github.com/quiclog/qvis),
  which is built into the image and served under `/qvis/`.

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
