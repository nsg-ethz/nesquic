#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

SLICE_CLIENT="nesquic-client.slice"
SLICE_SERVER="nesquic-server.slice"
SERVER_ADDR="10.0.0.2:4433"
VETH_MM="veth-mm"
VETH_METRICS="veth-metrics"
CPU_ALL=0-39
CPU_SYSTEM=0-9,11-39
CPU_CLIENT=9
CPU_SERVER=10

WORKSPACE=$(dirname "$(readlink -f "$0")")/..
BIN="${WORKSPACE}/target/release/nesquic"
RES_DIR="${WORKSPACE}/res"

function may_fail {
    ($@ > /dev/null 2>&1) || true
}

function wait_for_pid {
    local pid=""
    while true; do
        pid=$(pgrep $1 | head -n1)
        if [[ -n "$pid" ]]; then
            echo "$pid"
            return 0
        fi
        sleep 0.1
    done
}

function run_server {
    echo -e "${COLOR_YELLOW}Starting $1 server${COLOR_OFF}"
    GATEWAY_IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' pushgateway)
    PR_PUSH_GATEWAY=http://${GATEWAY_IP}:9091 mm-delay $3 systemd-run -q --scope --slice ${SLICE_SERVER} ${BIN} server -j $2 --lib $1 --cert ${RES_DIR}/pem/cert.pem --key ${RES_DIR}/pem/key.pem 0.0.0.0:4433 &

    echo -e "${COLOR_YELLOW}Add metrics link${COLOR_OFF}"
    SERVER_PID=$(wait_for_pid nesquic)
    sudo ip link set ${VETH_METRICS} netns ${SERVER_PID}
    sudo nsenter -t ${SERVER_PID} -n ip addr add ${DK_SUBNET} dev ${VETH_METRICS}
    sudo nsenter -t ${SERVER_PID} -n ip link set ${VETH_METRICS} up
}

function run_client {
    echo -e "${COLOR_YELLOW}Starting $1 client${COLOR_OFF}"
    systemd-run -q --scope --slice ${SLICE_CLIENT} ${BIN} client -j $2 --lib $1 --cert ${RES_DIR}/pem/cert.pem --blob $3 http://${SERVER_ADDR}
}

function cpu_governor {
    echo -e "${COLOR_YELLOW}Set CPU governor: $1${COLOR_OFF}"
    echo $1 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
}

# removes namespace upon failure or end of script
function teardown {
    may_fail sudo killall -s SIGINT nesquic
    may_fail sudo ip link ip link delete ${VETH_MM}
    sudo chmod u-s $(which systemd-run)

    cpu_governor "schedutil"

    echo -e "${COLOR_YELLOW}Resetting CPU isolation${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_ALL}
}

function setup {
    may_fail sudo ip link ip link delete ${VETH_MM}
    may_fail sudo killall nesquic
    sudo chmod u+s $(which systemd-run)

    # compile IUTs in release mode
    echo -e "${COLOR_YELLOW}Compile Nesquic${COLOR_OFF}"
    cargo build --release --bin nesquic

    cpu_governor "performance"

    sudo ip link add ${VETH_MM} type veth peer name veth-metrics
    sudo ip link set ${VETH_MM} up
    sudo brctl addif ${DK_BRIDGE} ${VETH_MM}

    echo -e "${COLOR_YELLOW}Isolating CPUs${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime ${SLICE_CLIENT} AllowedCPUs=${CPU_CLIENT}
    sudo systemctl set-property --runtime ${SLICE_SERVER} AllowedCPUs=${CPU_SERVER}
}

trap teardown EXIT INT TERM
setup

echo -e "${COLOR_YELLOW}Benchmarking quinn->quinn${COLOR_OFF}"
run_server quinn unbounded 0 0 0
run_client quinn unbounded 10Mbit
echo -e "${COLOR_GREEN}Done${COLOR_OFF}"

# echo -e "${COLOR_YELLOW}Benchmarking msquic->msquic${COLOR_OFF}"
# runb ${SLICE_SERVER} ${BIN} server --lib msquic --cert ${RES_DIR}/pem/cert.pem --key ${RES_DIR}/pem/key.pem
# run ${SLICE_CLIENT} ${BIN} client --lib msquic --cert ${RES_DIR}/pem/cert.pem --blob 1Mbit http://127.0.0.1:4433
# echo -e "${COLOR_GREEN}Done${COLOR_OFF}"

# echo -e "${COLOR_YELLOW}Benchmarking quiche->quiche${COLOR_OFF}"
# runb ${SLICE_SERVER} ${BIN} server --lib quiche --cert ${RES_DIR}/pem/cert.pem --key ${RES_DIR}/pem/key.pem
# run ${SLICE_CLIENT} ${BIN} client --lib quiche --cert ${RES_DIR}/pem/cert.pem --blob 1Mbit http://127.0.0.1:4433
# echo -e "${COLOR_GREEN}Done${COLOR_OFF}"
