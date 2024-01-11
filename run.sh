#!/bin/bash

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
CLIENT="iuts/quinn/target/debug/examples/client"
SERVER="iuts/quinn/target/debug/examples/server"

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
nsexec tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 1000ms

echo Compile IUT
cargo build --bin server --bin client

echo Start server in background
nsexec ${SERVER} --cert res/cert.der --key res/key.der iuts/quinn &> /dev/null &

echo Start client
nsexec perf record ${CLIENT} --ca res/cert.der https://localhost:4433/README.md &> /dev/null
kill %1