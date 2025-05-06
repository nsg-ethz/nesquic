#!/bin/bash

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

CLIENT_BIN="target/release/qbench"
SERVER_BIN="target/release/$1-server"
CPU_SYSTEM=0,1,20,21
CPU_QBENCH=2-19,22-39
TASKSET="taskset -c ${CPU_QBENCH}"

function run {
    sudo -E systemd-run -q --scope -u $1 --slice qbench.slice ${@:2}
}

function runb {
    sudo -b -E systemd-run -q --scope -u $1 --slice qbench.slice ${@:2}
}

function stop_probes {
    sudo killall -SIGINT funclatency-bpfcc >/dev/null 2>&1
}

function cleanup {
    stop_probes
    sudo systemctl stop server.scope > /dev/null 2>&1
    sudo systemctl stop client.scope > /dev/null 2>&1
}

echo -e "${COLOR_YELLOW}Assigning CPUs ${CPU_QBENCH} to experiment${COLOR_OFF}"
sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime qbench.slice AllowedCPUs=${CPU_QBENCH}

echo Start server in background
runb server ${SERVER_BIN} --cert res/pem/cert.pem --key res/pem/key.pem
sleep 1
SERVER_PID=$(pidof $1-server)

sudo -b funclatency-bpfcc -p ${SERVER_PID} ${SERVER_BIN}:"*rustls*" > res/runs/rustls.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} -r "^vfs_writev?$" > res/runs/write.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} -r "^vfs_readv?$" > res/runs/read.log 2>/dev/null

sleep 3

echo Start client
run client ${CLIENT_BIN} --cert res/pem/cert.pem --blob 500Mbit --reps 30 https://127.0.0.1:4433

cleanup
