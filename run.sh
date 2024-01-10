#!/bin/bash
# set -x

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
CLIENT="iuts/quinn/target/debug/examples/client"
SERVER="iuts/quinn/target/debug/examples/server"

cleanup() {
    echo Remove network namespace
    sudo ip netns del ${NETNS}
}
trap cleanup EXIT

echo Create testing network
sudo ip netns add ${NETNS}
sudo ip netns exec ${NETNS} ip link set dev ${IFACE} up
sudo ip netns exec ${NETNS} tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 1000ms

echo Compile IUT
cd iuts/quinn
cargo build --example server --example client
cd ../..

echo Start server in background
sudo ip netns exec ${NETNS} ${SERVER} iuts/quinn &> /dev/null &

echo Start client
sudo ip netns exec ${NETNS} ${CLIENT} https://localhost:4433/README.md > /dev/null
kill %1