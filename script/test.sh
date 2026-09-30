#!/usr/bin/env bash
#
# Connectivity smoke test for a QUIC IUT, mirroring `test::connectivity`
# (iut/common/src/test.rs): start the server container, wait until it becomes
# reachable, then run the client container and assert the transfer succeeds.
#
# It also asserts that libnesquic.so sees the IUT's packets through its crypto
# hooks: the client must report QUIC packet counts, TTFB and request latency,
# and both sides must write a qlog trace (NQ_QLOG) with sent and received
# packets.
#
# Runs the IUT inside its docker image (nesquic/<library>) so the test is
# language independent and exercises the same artifact CI ships. The MM_* knobs
# are left unset, so mm-entrypoint.sh runs the binary directly without mahimahi
# network emulation (see docker/mm-entrypoint.sh).
#
# Usage:
#   script/test.sh <library>        # e.g. quinn, quiche, ngtcp2, lsquic
#
# Environment overrides:
#   PORT      UDP port                              (default 4433)
#   BLOB      payload requested by the client       (default 50Mbit)
#   ATTEMPTS  connection attempts before giving up  (default 30)
#   TIMEOUT   per-attempt client timeout in seconds (default 10)

set -u

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m'

WORKSPACE="$(dirname "$(readlink -f "$0")")/.."

LIB="${1:-}"
if [[ -z "${LIB}" ]]; then
    echo "usage: $0 <library>" >&2
    exit 2
fi

DOCKERFILE="${WORKSPACE}/docker/Dockerfile.${LIB}"
if [[ ! -f "${DOCKERFILE}" ]]; then
    echo -e "${COLOR_RED}error: no docker/Dockerfile.${LIB}${COLOR_OFF}" >&2
    exit 2
fi

PORT="${PORT:-4433}"
BLOB="${BLOB:-1Mbit}"
ATTEMPTS="${ATTEMPTS:-30}"
TIMEOUT="${TIMEOUT:-10}"
IMAGE="nesquic/${LIB}"
SERVER_CONTAINER="nesquic-test-server-${LIB}"
# Certificates baked into the mahimahi base image (see docker/Dockerfile.mahimahi).
CERT="/workspace/res/pem/cert.pem"
KEY="/workspace/res/pem/key.pem"
URL="https://127.0.0.1:${PORT}"

QLOG_DIR="$(mktemp -d)"
chmod 777 "${QLOG_DIR}"

function cleanup {
    docker rm -f "${SERVER_CONTAINER}" >/dev/null 2>&1 || true
}

function finish {
    cleanup
    rm -rf "${QLOG_DIR}" 2>/dev/null || true
}
trap finish EXIT INT TERM

cleanup
echo -e "${COLOR_YELLOW}Starting ${LIB} server on 127.0.0.1:${PORT}${COLOR_OFF}"
# Host networking lets the client reach the server on the loopback address;
# MM_* env vars are deliberately unset so mahimahi is not activated.
docker run -d --network=host --name "${SERVER_CONTAINER}" \
    -v "${QLOG_DIR}:/qlog" -e NQ_QLOG=/qlog/server.qlog "${IMAGE}" \
    server --cert "${CERT}" --key "${KEY}" "127.0.0.1:${PORT}" >/dev/null \
    || { echo -e "${COLOR_RED}error: failed to start server container${COLOR_OFF}" >&2; exit 1; }

# Poll the server with real client connections until one succeeds, mirroring the
# health-check loop in test::connectivity. A successful client run is itself the
# connectivity assertion (connect + transfer the blob).
healthy=false
client_log=""
for ((i = 1; i <= ATTEMPTS; i++)); do
    if [[ -z "$(docker ps -q --filter "name=${SERVER_CONTAINER}")" ]]; then
        echo -e "${COLOR_RED}error: server exited before becoming reachable${COLOR_OFF}" >&2
        docker logs "${SERVER_CONTAINER}" 2>&1 || true
        exit 1
    fi

    rm -f "${QLOG_DIR}/client.qlog"
    if client_log="$(timeout -k 5 "${TIMEOUT}" docker run --rm --network=host \
            -v "${QLOG_DIR}:/qlog" -e NQ_QLOG=/qlog/client.qlog "${IMAGE}" \
            client "${URL}" --cert "${CERT}" --blob "${BLOB}" 2>&1)"; then
        healthy=true
        break
    fi

    sleep 0.3
done

if [[ "${healthy}" == true ]]; then
    echo -e "${COLOR_GREEN}ok: ${LIB} client and server connected (${BLOB} transferred)${COLOR_OFF}"

    # Stop the server gracefully so that it finishes its qlog trace.
    docker stop --time 5 "${SERVER_CONTAINER}" >/dev/null 2>&1 || true

    failed=false
    if grep -q '^nesquic_quic,' <<< "${client_log}"; then
        echo -e "${COLOR_GREEN}ok: ${LIB} client reported QUIC packet counts${COLOR_OFF}"
    else
        echo -e "${COLOR_RED}fail: ${LIB} client reported no QUIC packet counts (crypto hooks)${COLOR_OFF}" >&2
        failed=true
    fi
    if latency="$(grep -m1 '^nesquic_latency,' <<< "${client_log}")" \
            && [[ "${latency}" == *ttfb_ms=* && "${latency}" == *request_latency_ms=* ]]; then
        echo -e "${COLOR_GREEN}ok: ${LIB} client reported TTFB and request latency${COLOR_OFF}"
    else
        echo -e "${COLOR_RED}fail: ${LIB} client reported no TTFB/request latency${COLOR_OFF}" >&2
        failed=true
    fi
    for side in client server; do
        qlog="${QLOG_DIR}/${side}.qlog"
        sent="$(grep -c 'packet_sent' "${qlog}" 2>/dev/null)"
        received="$(grep -c 'packet_received' "${qlog}" 2>/dev/null)"
        if [[ "${sent:-0}" -gt 0 && "${received:-0}" -gt 0 ]]; then
            echo -e "${COLOR_GREEN}ok: ${LIB} ${side} qlog: ${sent} packets sent, ${received} received${COLOR_OFF}"
        else
            echo -e "${COLOR_RED}fail: ${LIB} ${side} qlog lacks sent/received packets${COLOR_OFF}" >&2
            failed=true
        fi
    done
    if [[ "${failed}" == true ]]; then
        echo "--- client output ---" >&2
        echo "${client_log}" >&2
        exit 1
    fi
    exit 0
fi

echo -e "${COLOR_RED}fail: ${LIB} client could not connect after ${ATTEMPTS} attempts${COLOR_OFF}" >&2
echo "--- last client output ---" >&2
echo "${client_log}" >&2
echo "--- server log ---" >&2
docker logs "${SERVER_CONTAINER}" 2>&1 || true
exit 1
