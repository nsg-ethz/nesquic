#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

BANDWIDTH="20mbit"
IFACE="lo"
NETNS="nesquic"

SLICE_CLIENT="nesquic-client.slice"
SLICE_SERVER="nesquic-server.slice"
CPU_ALL=0-39
CPU_SYSTEM=0-9,11-39
CPU_CLIENT=9
CPU_SERVER=10

WORKSPACE=$(dirname "$(readlink -f "$0")")/..
BIN="${WORKSPACE}/target/release/nesquic"

function may_fail {
    ($@ > /dev/null 2>&1) || true
}

function runb {
    sudo -b -E systemd-run -q --scope --slice $1 ip netns exec ${NETNS} ${@:2}
}

function run {
    sudo -E systemd-run -q --scope --slice $1 ip netns exec ${NETNS} ${@:2}
}

function cpu_governor {
    echo -e "${COLOR_YELLOW}Set CPU governor: $1${COLOR_OFF}"
    echo $1 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
}

# removes namespace upon failure or end of script
function teardown {
    may_fail sudo killall nesquic

    cpu_governor "schedutil"

    echo -e "${COLOR_YELLOW}Remove network namespace${COLOR_OFF}"
    sudo ip netns del ${NETNS}

    echo -e "${COLOR_YELLOW}Resetting CPU isolation${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_ALL}
}

function setup {
    may_fail sudo killall nesquic

    echo -e "${COLOR_YELLOW}Setup benchmarking network${COLOR_OFF}"
    may_fail sudo ip netns del ${NETNS}
    sudo ip netns add ${NETNS}
    sudo ip netns exec ${NETNS} ip link set dev ${IFACE} up
    # nsexec tc qdisc add dev ${IFACE} root netem rate ${BANDWIDTH} delay 50ms

    # compile IUTs in release mode
    echo -e "${COLOR_YELLOW}Compile Nesquic${COLOR_OFF}"
    cargo build --release --bin nesquic

    cpu_governor "performance"

    echo -e "${COLOR_YELLOW}Isolating CPUs${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime ${SLICE_CLIENT} AllowedCPUs=${CPU_CLIENT}
    sudo systemctl set-property --runtime ${SLICE_SERVER} AllowedCPUs=${CPU_SERVER}
}

trap teardown EXIT INT TERM
setup

echo -e "${COLOR_YELLOW}BENCHMARKING quinn->quinn${COLOR_OFF}"
runb ${SLICE_SERVER} ${BIN} server --lib quinn --cert res/pem/cert.pem --key res/pem/key.pem
run ${SLICE_CLIENT} ${BIN} client --lib quinn --cert res/pem/cert.pem --blob 1Mbit http://127.0.0.1:4433
echo -e "${COLOR_GREEN}Done${COLOR_OFF}"

echo -e "${COLOR_YELLOW}BENCHMARKING msquic->msquic${COLOR_OFF}"
runb ${SLICE_SERVER} ${BIN} server --lib msquic --cert res/pem/cert.pem --key res/pem/key.pem
run ${SLICE_CLIENT} ${BIN} client --lib msquic --cert res/pem/cert.pem --blob 1Mbit http://127.0.0.1:4433
echo -e "${COLOR_GREEN}Done${COLOR_OFF}"

echo -e "${COLOR_YELLOW}BENCHMARKING quiche->quiche${COLOR_OFF}"
runb ${SLICE_SERVER} ${BIN} server --lib quiche --cert res/pem/cert.pem --key res/pem/key.pem
run ${SLICE_CLIENT} ${BIN} client --lib quiche --cert res/pem/cert.pem --blob 1Mbit http://127.0.0.1:4433
echo -e "${COLOR_GREEN}Done${COLOR_OFF}"
