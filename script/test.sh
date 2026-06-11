#!/usr/bin/env bash
#
# Connectivity smoke test for a QUIC IUT, mirroring `test::connectivity`
# (iut/common/src/test.rs): start the server container, wait until it becomes
# reachable, then run the client container and assert the transfer succeeds.
#
# Runs the IUT inside its docker image (nesquic/<library>) so the test is
# language independent and exercises the same artifact CI ships. The MM_* knobs
# are left unset, so mm-entrypoint.sh runs the binary directly without mahimahi
# network emulation (see docker/mm-entrypoint.sh).
#
# Usage:
#   script/test.sh <library>        # e.g. quinn, quiche, neqo, noq, msquic
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

function cleanup {
    docker rm -f "${SERVER_CONTAINER}" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

cleanup
echo -e "${COLOR_YELLOW}Starting ${LIB} server on 127.0.0.1:${PORT}${COLOR_OFF}"
# Host networking lets the client reach the server on the loopback address;
# MM_* env vars are deliberately unset so mahimahi is not activated.
docker run -d --network=host --name "${SERVER_CONTAINER}" "${IMAGE}" \
    server --cert "${CERT}" --key "${KEY}" "127.0.0.1:${PORT}" >/dev/null \
    || { echo -e "${COLOR_RED}error: failed to start server container${COLOR_OFF}" >&2; exit 1; }

# Poll the server with real client connections until one succeeds, mirroring the
# health-check loop in test::connectivity. A successful client run is itself the
# connectivity assertion (connect + transfer the blob).
healthy=false
for ((i = 1; i <= ATTEMPTS; i++)); do
    if [[ -z "$(docker ps -q --filter "name=${SERVER_CONTAINER}")" ]]; then
        echo -e "${COLOR_RED}error: server exited before becoming reachable${COLOR_OFF}" >&2
        docker logs "${SERVER_CONTAINER}" 2>&1 || true
        exit 1
    fi

    if timeout "${TIMEOUT}" docker run --rm --network=host "${IMAGE}" \
            client "${URL}" --cert "${CERT}" --blob "${BLOB}" >/dev/null 2>&1; then
        healthy=true
        break
    fi

    sleep 0.3
done

if [[ "${healthy}" == true ]]; then
    echo -e "${COLOR_GREEN}ok: ${LIB} client and server connected (${BLOB} transferred)${COLOR_OFF}"
    exit 0
fi

echo -e "${COLOR_RED}fail: ${LIB} client could not connect after ${ATTEMPTS} attempts${COLOR_OFF}" >&2
echo "--- server log ---" >&2
docker logs "${SERVER_CONTAINER}" 2>&1 || true
exit 1
