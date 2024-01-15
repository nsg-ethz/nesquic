#!/bin/bash

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
CLIENT_BIN="target/release/client"
SERVER_BIN="target/release/server"
PERF_CMD="perf record -F 100 -g -a"
PERF_OUT="res/out.perf"
FLAME_DIR="../../tools/FlameGraph"

# stop the script if an error occurs
set -e

# execute command in namespace
nsexec() {
    sudo ip netns exec ${NETNS} $@
}

# removes namespace upon failure or end of script
cleanup() {
    echo Remove network namespace
    sudo ip netns del ${NETNS}
}
trap cleanup EXIT

# create a new networking namespace
# this allows us to ratelimit the lo device without slowing down the entire VM
echo Create testing network
sudo ip netns add ${NETNS}
nsexec ip link set dev ${IFACE} up
# nsexec tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 1000ms

# compile IUTs in release mode
echo Compile IUT
cargo build --release --bin server --bin client

echo Start server in background
nsexec ${SERVER_BIN} --cert res/ca/cert.der --key res/ca/key.der &> /dev/null &

echo Start client
nsexec ${PERF_CMD} ${CLIENT_BIN} --cert res/ca/cert.der https://localhost:4433/20Gbit
kill %1

sudo perf script > ${PERF_OUT}
sudo rm perf.data
chown ${USER} ${PERF_OUT}

echo Render flame graph
${FLAME_DIR}/stackcollapse-perf.pl ${PERF_OUT} > out.folded
mv out.folded ${PERF_OUT}

${FLAME_DIR}/flamegraph.pl ${PERF_OUT} > res/flame.svg