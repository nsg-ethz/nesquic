#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

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

function run_client {
    MAHIMAHI_BASE="10.0.0.1"
    CMD="PR_PUSH_GATEWAY=http://${MAHIMAHI_BASE}:9091 "
    CMD+="mm-delay ${EXP_DELAY} "

    if [ "${EXP_LOSS}" -gt 0 ]; then
        CMD+="mm-loss uplink ${EXP_LOSS} "
    fi

    if [ -n "${EXP_LINK}" ]; then
        CMD+="mm-link ${EXP_LINK}.up ${EXP_LINK}.down -- "
    fi

    CMD+="${BIN} client -j \"${EXP_NAME}\" --lib $1 --cert ${RES_DIR}/pem/cert.pem --blob ${EXP_BLOB} --quic-cpu 8 --metric-cpu 9 https://${MAHIMAHI_BASE}:4433"

    eval ${CMD}
}

function run_server {
    ${BIN} server -j "${EXP_NAME}" --lib $1 --cert ${RES_DIR}/pem/cert.pem --key ${RES_DIR}/pem/key.pem 0.0.0.0:4433 --quic-cpu 10 --metric-cpu 11 &
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

    echo -e "${COLOR_YELLOW}Setting up firewall${COLOR_OFF}"
    sudo ufw allow from 10.0.0.0/24 to any port 9901
    sudo ufw allow from 10.0.0.0/24 to any port 4433

    cpu_governor "performance"

    echo -e "${COLOR_YELLOW}Isolating CPUs${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
}

function reset_exp {
    EXP_NAME=""
    EXP_DELAY=0
    EXP_LOSS=0
    EXP_LINK=""
    EXP_BLOB=""
}

function config_exp_unbounded {
    reset_exp
    EXP_NAME="unbounded"
    EXP_BLOB="50Mbit"
}

function config_exp_short_delay {
    reset_exp
    EXP_NAME="5ms delay"
    EXP_DELAY=5
    EXP_BLOB="50Mbit"
}

function config_exp_long_delay {
    reset_exp
    EXP_NAME="20ms delay"
    EXP_DELAY=20
    EXP_BLOB="50Mbit"
}

function config_exp_driving {
    reset_exp
    EXP_NAME="driving"
    EXP_DELAY=50
    EXP_LINK="${WORKSPACE}/res/traces/TMobile-LTE-driving"
    EXP_BLOB="10Mbit"
}

function run_experiment {
    echo -ne "run ${EXP_NAME}... "

    run_server $1
    wait_for_launch nesquic > /dev/null 2>&1
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

    config_exp_driving
    run_experiment $1

    echo -e "${COLOR_GREEN}Done${COLOR_OFF}"
}

# check if the pushgateway is running
docker ps --filter "name=pushgateway" --filter "status=running" --format '{{.Names}}' | grep -wq pushgateway
if [ $? -ne 0 ]; then
  echo -e "${COLOR_RED}Pushgateway is not running${COLOR_OFF}"
  exit 1
fi

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
