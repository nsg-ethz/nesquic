#!/usr/bin/env bash

set -eu

LIB="${1:-}"
if [[ -z "${LIB}" ]]; then
    echo "usage: $0 <library>" >&2
    exit 2
fi

WORKSPACE=$(dirname "$(readlink -f "$0")")/..

docker build -f ${WORKSPACE}/docker/Dockerfile.rust -t nesquic/rust ${WORKSPACE}
docker build -f ${WORKSPACE}/docker/Dockerfile.mahimahi -t nesquic/mahimahi ${WORKSPACE}
docker build -f ${WORKSPACE}/docker/Dockerfile.preload -t nesquic/preload ${WORKSPACE}

# Libraries with slow-to-build dependencies keep them in a separate image.
if [[ -f ${WORKSPACE}/docker/Dockerfile.${LIB}-deps ]]; then
    docker build -f ${WORKSPACE}/docker/Dockerfile.${LIB}-deps -t nesquic/${LIB}-deps ${WORKSPACE}
fi

docker build -f ${WORKSPACE}/docker/Dockerfile.${LIB} -t nesquic/${LIB} ${WORKSPACE}
