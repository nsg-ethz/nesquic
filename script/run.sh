#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

VETH_MM="veth-mm"
VETH_METRICS="veth-metrics"

CPU_ALL=0-39
CPU_SYSTEM=0-7,12-39
NUM_CPU=8

NESQUIC_BENCHMARK=0

WORKSPACE=$(dirname "$(readlink -f "$0")")/..
RES_DIR="${WORKSPACE}/res"

NESQUIC_RUN_LABEL="${NESQUIC_RUN_LABEL:-default}"

# Names of containers currently running (set by run_server / run_client)
SERVER_CONTAINER=""
CLIENT_CONTAINER=""

function may_fail {
    ($@ > /dev/null 2>&1) || true
}

function wait_for_launch {
    local printed=false
    while true; do
        if docker ps --filter "name=${SERVER_CONTAINER}" --filter "status=running" \
               --format "{{.Names}}" 2>/dev/null | grep -q .; then
            return 0
        fi
        if [[ ! $printed ]]; then
            echo "Waiting for ${SERVER_CONTAINER}..."
            printed=true
        fi
        sleep 0.1
    done
}

function wait_for_term {
    while docker ps --filter "name=${SERVER_CONTAINER}" --filter "status=running" \
              --format "{{.Names}}" 2>/dev/null | grep -q .; do
        sleep 0.1
    done
}

function run_client {
    CLIENT_CONTAINER="nesquic-client-$1"

    may_fail docker rm -f ${CLIENT_CONTAINER}

    LOCALHOST_IP="127.0.0.1"
    if [[ -n "${EXP_DELAY}" || -n "${EXP_LOSS}" || -n "${EXP_LINK}" ]]; then
        LOCALHOST_IP="10.0.0.1" # MAHIMAHI_BASE
    fi

    docker run --rm --network=host \
        --user $(id -u):$(id -g) \
        --cap-add=NET_ADMIN \
        --cap-add=SYS_ADMIN \
        --device=/dev/net/tun \
        -e MM_DELAY=${EXP_DELAY} \
        -e MM_LOSS=${EXP_LOSS} \
        -e MM_LINK=${EXP_LINK} \
        -e INFLUX_URL=http://${LOCALHOST_IP}:8086 \
        -e INFLUX_TOKEN=${INFLUX_TOKEN:-nesquic-token} \
        -e INFLUX_ORG=${INFLUX_ORG:-nesquic} \
        -e INFLUX_BUCKET=${INFLUX_BUCKET:-nesquic} \
        --name ${CLIENT_CONTAINER} \
        nesquic/$1 \
        client -j ${EXP_NAME} --cert /workspace/res/pem/cert.pem --blob ${EXP_BLOB} \
        https://${LOCALHOST_IP}:4433 -L nesquic_run:${NESQUIC_RUN_LABEL}
}

function run_server {
    SERVER_CONTAINER="nesquic-server-$1"

    # Remove any stale container with the same name
    may_fail docker rm -f ${SERVER_CONTAINER}

    CMD="docker run --rm --network=host "
    CMD+="--user $(id -u):$(id -g) "
    CMD+="--name ${SERVER_CONTAINER} "
    CMD+="-e INFLUX_URL=http://127.0.0.1:8086 "
    CMD+="-e INFLUX_TOKEN=${INFLUX_TOKEN:-nesquic-token} "
    CMD+="-e INFLUX_ORG=${INFLUX_ORG:-nesquic} "
    CMD+="-e INFLUX_BUCKET=${INFLUX_BUCKET:-nesquic} "
    CMD+="nesquic/$1 "
    CMD+="server -j ${EXP_NAME} --cert /workspace/res/pem/cert.pem --key /workspace/res/pem/key.pem 0.0.0.0:4433  -L nesquic_run:${NESQUIC_RUN_LABEL} &"

    eval ${CMD}
}

function kill_nesquic {
    if [ -n "${SERVER_CONTAINER}" ]; then
        may_fail docker stop --time 2 ${SERVER_CONTAINER}
    fi
    if [ -n "${CLIENT_CONTAINER}" ]; then
        may_fail docker stop --time 2 ${CLIENT_CONTAINER}
    fi
}

function cpu_governor {
    echo -e "${COLOR_YELLOW}Set CPU governor: $1${COLOR_OFF}"
    echo $1 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
}

function teardown {
    kill_nesquic KILL

    # Stop all nesquic containers in case teardown is called mid-run
    may_fail docker stop $(docker ps -q --filter "name=nesquic-server-") 2>/dev/null
    may_fail docker stop $(docker ps -q --filter "name=nesquic-client-") 2>/dev/null

    may_fail sudo ip link del ${VETH_MM}

    if [ ${NESQUIC_BENCHMARK} -eq 1 ]; then
        cpu_governor "schedutil"
    fi

    echo -e "${COLOR_YELLOW}Resetting CPU isolation${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_ALL}

    exit 0
}

function setup {
    kill_nesquic KILL
    may_fail sudo ip link del ${VETH_MM}

    echo -e "${COLOR_YELLOW}Setting up firewall${COLOR_OFF}"
    sudo ufw allow from 10.0.0.0/24 to any port 8086
    sudo ufw allow from 10.0.0.0/24 to any port 4433

    if [ ${NESQUIC_BENCHMARK} -eq 1 ]; then
        cpu_governor "performance"
    fi

    echo -e "${COLOR_YELLOW}Isolating CPUs${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
}

function reset_exp {
    EXP_NAME=
    EXP_DELAY=
    EXP_LOSS=
    EXP_LINK=
    EXP_BLOB=
}

function config_exp_unbounded {
    reset_exp
    EXP_NAME="unbounded"
    EXP_BLOB="50Mbit"
}

function config_exp_short_delay {
    reset_exp
    EXP_NAME="delay5"
    EXP_DELAY=5
    EXP_BLOB="50Mbit"
}

function config_exp_long_delay {
    reset_exp
    EXP_NAME="delay20"
    EXP_DELAY=20
    EXP_BLOB="50Mbit"
}

function config_exp_driving {
    reset_exp
    EXP_NAME="driving"
    EXP_DELAY=50
    EXP_LINK="TMobile-LTE-driving"
    EXP_BLOB="50Mbit"
}

function run_experiment {
    echo -e "run ${EXP_NAME}... "

    run_server $1
    wait_for_launch
    run_client $1

    # kill server and give it time to upload its metrics
    kill_nesquic
    wait_for_term

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
docker ps --filter "name=influxdb" --filter "status=running" --format '{{.Names}}' | grep -wq influxdb
if [ $? -ne 0 ]; then
  echo -e "${COLOR_RED}InfluxDB is not running${COLOR_OFF}"
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
    ${WORKSPACE}/script/build.sh ${LIB}
    run_library_experiments ${LIB}
done

teardown
