#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

# Parse arguments
while getopts "n:i:" opt; do
    case $opt in
        n ) NAME=${OPTARG} ;;
        i ) IUT=${OPTARG} ;;
        \?)
            echo "Invalid option: -$OPTARG"
            ;;
    esac
done

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="qbench"
ROOT=$(dirname "$(readlink -f "$0")")
SUMMARY_DIR=${ROOT}/../res/runs/${NAME}

mkdir -p ${SUMMARY_DIR}

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
# nsexec tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 50ms

# compile IUTs in release mode
echo Compile IUT
cargo build --release --bin ${IUT}-server --bin qbench

${ROOT}/conn.sh -n ${NAME} -i ${IUT}
