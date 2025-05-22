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

    echo -e "${COLOR_YELLOW}Resetting CPU governor${COLOR_OFF}"
    echo schedutil | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
}

cleanup

echo -e "${COLOR_YELLOW}Setting CPU governor${COLOR_OFF}"
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

echo -e "${COLOR_YELLOW}Assigning CPUs ${CPU_QBENCH} to experiment${COLOR_OFF}"
sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
sudo systemctl set-property --runtime qbench.slice AllowedCPUs=${CPU_QBENCH}

echo Start server in background
if [ ${IUT} = "msquic" ]; then
    runb qb-server ${SERVER_BIN} -server -cert_file:res/pem/cert.pem -key_file:res/pem/key.pem > /dev/null 2>&1
else
    runb qb-server ${SERVER_BIN} --cert res/pem/cert.pem --key res/pem/key.pem > /dev/null 2>&1
fi

sleep 1
SERVER_PID=$(pidof ${IUT}-server)

sudo -b systemd-run -q --scope -u qb-cpu ${ROOT}/capture-cpu.sh -n ${NAME} -i ${IUT}
sudo -b funclatency-bpfcc -p ${SERVER_PID} ${SERVER_BIN}:"*rustls*crypt_in_place*" > ${SUMMARY_DIR}/${IUT}-bpf-rustls.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} msquic:"CxPlat..crypt" > ${SUMMARY_DIR}/${IUT}-bpf-quictls.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "__sys_sendmmsg" > ${SUMMARY_DIR}/${IUT}-bpf-sendmmsg.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "__sys_sendmsg" > ${SUMMARY_DIR}/${IUT}-bpf-sendmsg.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "__sys_sendto" > ${SUMMARY_DIR}/${IUT}-bpf-sendto.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "do_recvmmsg" > ${SUMMARY_DIR}/${IUT}-bpf-recvmmsg.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "__sys_recvfrom" > ${SUMMARY_DIR}/${IUT}-bpf-recvfrom.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "process_backlog" > ${SUMMARY_DIR}/${IUT}-bpf-ipc.log 2>/dev/null
sudo -b funclatency-bpfcc -p ${SERVER_PID} "ep_send_events" > ${SUMMARY_DIR}/${IUT}-bpf-epoll.log 2>/dev/null

sleep 3

echo Start client

run client ${CLIENT_BIN} --cert res/pem/cert.pem --blob 200Mbit --reps 50 https://127.0.0.1:4433 > ${SUMMARY_DIR}/${IUT}-qbench.log 2>&1

echo -e "${COLOR_GREEN}Done${COLOR_OFF}"
cat ${SUMMARY_DIR}/${IUT}-qbench.log

cleanup
