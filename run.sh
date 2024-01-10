#!/bin/bash
# set -x

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
CLIENT="iuts/quinn/target/debug/examples/client"
SERVER="iuts/quinn/target/debug/examples/server"

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
cd iuts/quinn
cargo build --example server --example client
cd ../..

echo Start server in background
nsexec ${SERVER} iuts/quinn &

nsexec ${CLIENT} https://localhost:4433/README.md
kill %1