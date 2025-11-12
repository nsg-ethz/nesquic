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
FILE=${SUMMARY_DIR}/${IUT}-cpu.log

mkdir -p ${SUMMARY_DIR}

function read_cpu_usage() {
    awk '$1 == "usage_usec" { print $2 }' /sys/fs/cgroup/nesquic.slice/cpu.stat
}

TS=$(date +%s%N)
CPU=$(read_cpu_usage)

echo "timestamp,CPUPerc" > ${FILE}

while sleep 1; do
    TS_NEW=$(date +%s%N)
    CPU_NEW=$(read_cpu_usage)

    RES=$(bc -l <<< "(${CPU_NEW} - ${CPU}) / (${TS_NEW} - ${TS}) * 1000")
    echo "${TS_NEW},${RES}" >> ${FILE}
    sync

    TS=${TS_NEW}
    CPU=${CPU_NEW}
done
