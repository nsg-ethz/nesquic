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

ROOT=$(dirname "$(readlink -f "$0")")
SUMMARY_DIR=${ROOT}/../res/runs/${NAME}

mkdir -p ${SUMMARY_DIR}

CLIENT_BIN="${ROOT}/../target/release/qbench"
SERVER_BIN="${ROOT}/../target/release/${IUT}-server"
CPU_SYSTEM=0-10,20-30
CPU_QBENCH=10-19,31-39

function runb {
    sudo -b -E systemd-run -q --scope -u $1 --slice qbench.slice ${@:2}
}

function run {
    sudo -E systemd-run -q --scope -u $1 ${@:2}
}

function stop_probes {
    sudo killall -SIGINT funclatency-bpfcc >/dev/null 2>&1
}

function cleanup {
    stop_probes
    sudo systemctl stop qb-server.scope > /dev/null 2>&1
    sudo systemctl stop qb-cpu.scope > /dev/null 2>&1
}

cleanup

echo -e "${COLOR_YELLOW}Assigning CPUs ${CPU_QBENCH} to experiment${COLOR_OFF}"
sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime qbench.slice AllowedCPUs=${CPU_QBENCH}

echo Start server in background
runb qb-server ${SERVER_BIN} --cert res/pem/cert.pem --key res/pem/key.pem
sleep 1
SERVER_PID=$(pidof ${IUT}-server)

# sudo -b systemd-run -q --scope -u qb-cpu ${ROOT}/capture-cpu.sh -n ${NAME} -i ${IUT}
# sudo -b funclatency-bpfcc -p ${SERVER_PID} ${SERVER_BIN}:"*rustls*" > ${SUMMARY_DIR}/${IUT}-rustls.log 2>/dev/null
    # sudo -b funclatency-bpfcc -p ${SERVER_PID} -r "^vfs_writev?$" > ${SUMMARY_DIR}/${IUT}-write.log 2>/dev/null
        # sudo -b funclatency-bpfcc -p ${SERVER_PID} -r "^vfs_readv?$" > ${SUMMARY_DIR}/${IUT}-read.log 2>/dev/null

sleep 3

echo Start client
run client ${CLIENT_BIN} --cert res/pem/cert.pem --blob 500Mbit --reps 30 https://127.0.0.1:4433

cleanup
