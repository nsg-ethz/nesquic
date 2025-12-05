#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

SERVER_ADDR="10.0.0.2:4433"
VETH_MM="veth-mm"
VETH_METRICS="veth-metrics"
CPU_ALL=0-39
CPU_SYSTEM=0-7,12-39

WORKSPACE=$(dirname "$(readlink -f "$0")")/..
BIN="${WORKSPACE}/target/release/nesquic"
RES_DIR="${WORKSPACE}/res"

function may_fail {
    ($@ > /dev/null 2>&1) || true
}

function wait_for_launch {
    local pid=""
    local printed=false
    while true; do
        pid=$(pgrep $1 | head -n1)
        if [[ -n "$pid" ]]; then
            echo "$pid"
            return 0
        fi
        if [[ ! $printed ]]; then
            echo "Waiting for $1..."
            printed=true
        fi
        sleep 0.1
    done
}

function wait_for_term {
    while true; do
        pid=$(pgrep $1 | head -n1)
        if [[ -z "$pid" ]]; then
            return 0
        fi
        sleep 0.1
    done
}

function push_gateway {
    GATEWAY_IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' pushgateway)
    echo http://${GATEWAY_IP}:9091
    return 0
}

function run_server {
    PR_PUSH_GATEWAY=$(push_gateway) mm-delay ${EXP_DELAY} ${BIN} server -j "${EXP_NAME}" --lib $1 --cert ${RES_DIR}/pem/cert.pem --key ${RES_DIR}/pem/key.pem 0.0.0.0:4433 --quic-cpu 10 --metric-cpu 11 &
}

function run_client {
    PR_PUSH_GATEWAY=$(push_gateway) ${BIN} client -j "${EXP_NAME}" --lib $1 --cert ${RES_DIR}/pem/cert.pem --blob ${EXP_BLOB} --quic-cpu 8 --metric-cpu 9 https://${SERVER_ADDR}
}

function kill_nesquic {
    may_fail sudo killall -s ${1:-INT} nesquic
}

function cpu_governor {
    echo -e "${COLOR_YELLOW}Set CPU governor: $1${COLOR_OFF}"
    echo $1 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
}

function teardown {
    kill_nesquic KILL
    may_fail sudo ip link del ${VETH_MM}

    cpu_governor "schedutil"

    echo -e "${COLOR_YELLOW}Resetting CPU isolation${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_ALL}

    exit 0
}

function setup {
    kill_nesquic KILL
    may_fail sudo ip link del ${VETH_MM}

    # compile IUTs in release mode
    echo -e "${COLOR_YELLOW}Compile Nesquic${COLOR_OFF}"
    cargo build --release --bin nesquic
    sudo chown root:root ${BIN}
    sudo chmod u+s,o+rx ${BIN}

    cpu_governor "performance"

    echo -e "${COLOR_YELLOW}Isolating CPUs${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
}

function config_exp_unbounded {
    EXP_NAME="unbounded"
    EXP_DELAY=0
    EXP_BLOB="50Mbit"
}

function config_exp_short_delay {
    EXP_NAME="5ms delay"
    EXP_DELAY=5
    EXP_BLOB="50Mbit"
}

function config_exp_long_delay {
    EXP_NAME="20ms delay"
    EXP_DELAY=20
    EXP_BLOB="50Mbit"
}

function run_experiment {
    echo -ne "run ${EXP_NAME}... "

    sudo ip link add ${VETH_MM} type veth peer name ${VETH_METRICS}
    sudo ip link set ${VETH_MM} up
    sudo brctl addif ${DK_BRIDGE} ${VETH_MM}

    run_server $1

    # wait for the server to start and then add the metrics interface
    # this allows the server to push its metrics without loss/delay
    SERVER_PID=$(wait_for_launch nesquic)
    sudo ip link set ${VETH_METRICS} netns ${SERVER_PID}
    sudo nsenter -t ${SERVER_PID} -n ip addr add ${DK_SUBNET} dev ${VETH_METRICS}
    sudo nsenter -t ${SERVER_PID} -n ip link set ${VETH_METRICS} up

    run_client $1

    # kill nesquic and give it time to upload its metrics
    kill_nesquic
    wait_for_term nesquic

    echo -e "${COLOR_GREEN}ok${COLOR_OFF}"
}

function run_library_experiments {
    echo -e "${COLOR_YELLOW}Benchmarking $1${COLOR_OFF}"

    config_exp_unbounded
    run_experiment $1

    config_exp_short_delay
    run_experiment $1

    config_exp_long_delay
    run_experiment $1

    echo -e "${COLOR_GREEN}Done${COLOR_OFF}"
}

setup
trap teardown INT TERM

if [ "$#" -eq 0 ]; then
    LIBS=(${NQ_LIBS})
else
    LIBS=("$@")
fi

for LIB in "${LIBS[@]}"; do
    run_library_experiments ${LIB}
done

teardown
