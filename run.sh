#!/bin/bash

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
CLIENT_BIN="target/release/client"
SERVER_BIN="target/release/server"

set -e

nsexec() {
    sudo ip netns exec ${NETNS} $@
}

cleanup() {
    echo Remove network namespace
    sudo ip netns del ${NETNS}
}
trap cleanup EXIT

echo Create testing network
sudo ip netns add ${NETNS}
nsexec ip link set dev ${IFACE} up
# nsexec tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 1000ms

echo Compile IUT
cargo build --release --bin server --bin client

echo Start server in background
nsexec ${SERVER_BIN} --cert res/cert.der --key res/key.der &> /dev/null &

echo Start client
nsexec perf record ${CLIENT_BIN} --cert res/cert.der https://localhost:4433/20Gbit 2>&1
kill %1

sudo chown $USER perf.data