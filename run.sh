#!/bin/bash

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
PERF_OUT="res/out.perf"
PERF_TMP="res/tmp.perf"
FLAME_DIR="../../bin/FlameGraph"
IUT=$1

if [ $# -lt 1 ] 
then 
    echo "Specify the IUT you want to benchmark"
    exit 1
fi
echo Running benchmarks for ${IUT}

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
# nsexec tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 100ms

# compile IUTs in release mode
echo Compile IUT
cargo build --release --bin ${IUT}-server --bin ${IUT}-client

nsexec bash conn.sh ${IUT}

# sudo mv perf.data ${PERF_OUT}
# sudo perf script -i ${PERF_OUT} > ${PERF_TMP}

# echo Render flame graph
# ${FLAME_DIR}/stackcollapse-perf.pl --all ${PERF_TMP} > folded.perf
# mv folded.perf ${PERF_TMP}

# ${FLAME_DIR}/flamegraph.pl --colors java --hash ${PERF_TMP} > res/flame.svg
# rm ${PERF_TMP}