#!/bin/bash

set -eu

COLOR_RED='\033[0;31m'
COLOR_GREEN='\033[0;32m'
COLOR_YELLOW='\033[0;33m'
COLOR_OFF='\033[0m' # No Color

VETH_MM="veth-mm"

CPU_ALL=0-39
CPU_SYSTEM=0-7,12-39
NUM_CPU=8

WORKSPACE=$(dirname "$(readlink -f "$0")")/..
RES_DIR="${WORKSPACE}/res"

NQ_RUN_LABEL="${NQ_RUN_LABEL:-default}"
NQ_INVOCATION=$(date +%s)

NQ_REPS="${NQ_REPS:-1}"
if [[ ! ${NQ_REPS} =~ ^[1-9][0-9]*$ ]]; then
    echo -e "${COLOR_RED}error: NQ_REPS must be a positive integer${COLOR_OFF}" >&2
    exit 1
fi

# One line of shell assignments (EXP_*) per experiment.
mapfile -t EXPERIMENTS < <(yq -r '.[] | [
        "EXP_NAME=\(.job | @sh)",
        "EXP_BLOB=\(.blob | @sh)",
        "EXP_CONNECTIONS=\(.connections // 1 | @sh)",
        "EXP_STREAMS=\(.streams // 1 | @sh)",
        "EXP_DURATION=\(.duration // "" | @sh)",
        "EXP_DELAY=\(.delay // "" | @sh)",
        "EXP_LOSS=\(.loss // "" | @sh)",
        "EXP_LINK=\(.link // "" | @sh)"
    ] | join(" ")' "${RES_DIR}/experiments.yaml")
if [[ ${#EXPERIMENTS[@]} -eq 0 ]]; then
    echo -e "${COLOR_RED}error: could not read experiments from ${RES_DIR}/experiments.yaml (needs yq)${COLOR_OFF}" >&2
    exit 1
fi

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

function mode_args {
    case ${EXP_MODE} in
        detached)
            echo "-v /dev/null:/etc/ld.so.preload:ro"
            ;;
        attached)
            echo "-e INFLUX_URL=http://$3:8086 -e INFLUX_TOKEN=${INFLUX_TOKEN:-nesquic-token}" \
                "-e INFLUX_ORG=${INFLUX_ORG:-nesquic} -e INFLUX_BUCKET=${INFLUX_BUCKET:-nesquic}"
            ;;
        qlog)
            mkdir -p ${RES_DIR}/qlog/$1
            echo "-v ${RES_DIR}/qlog/$1:/workspace/qlog -e NQ_QLOG=/workspace/qlog/${EXP_NAME}.$2.qlog"
            ;;
    esac
}

function run_client {
    CLIENT_CONTAINER="nesquic-client-$1"

    may_fail docker rm -f ${CLIENT_CONTAINER}

    LOCALHOST_IP="127.0.0.1"
    if [[ -n "${EXP_DELAY}" || -n "${EXP_LOSS}" || -n "${EXP_LINK}" ]]; then
        # Expanded by mm-entrypoint.sh: mahimahi's base is not 10.0.0.1 if the
        # host already uses that address, and the server answers from the base.
        LOCALHOST_IP='$MAHIMAHI_BASE'
    fi

    # Without InfluxDB, libnesquic.so prints its metrics instead.
    local out=/dev/stdout
    # The qlog trace covers a single round of requests, to keep it small.
    local duration=${EXP_DURATION:+-d ${EXP_DURATION}}
    if [[ ${EXP_MODE} == qlog ]]; then
        out=/dev/null
        duration=
    fi

    docker run --rm --network=host \
        --user $(id -u):$(id -g) \
        --cap-add=NET_ADMIN \
        --cap-add=SYS_ADMIN \
        --device=/dev/net/tun \
        -e MM_DELAY=${EXP_DELAY} \
        -e MM_LOSS=${EXP_LOSS} \
        -e MM_LINK=${EXP_LINK} \
        --name ${CLIENT_CONTAINER} \
        $(mode_args $1 client ${LOCALHOST_IP}) \
        nesquic/$1 \
        client -j ${EXP_NAME} --cert /workspace/res/pem/cert.pem --blob ${EXP_BLOB} \
        -c ${EXP_CONNECTIONS} -s ${EXP_STREAMS} ${duration} \
        https://${LOCALHOST_IP}:4433 -L nesquic_run:${NQ_RUN_LABEL} -L nesquic_invocation:${NQ_INVOCATION} > ${out}
}

function run_server {
    SERVER_CONTAINER="nesquic-server-$1"

    # Remove any stale container with the same name
    may_fail docker rm -f ${SERVER_CONTAINER}

    CMD="docker run --rm --network=host "
    CMD+="--user $(id -u):$(id -g) "
    CMD+="--name ${SERVER_CONTAINER} "
    CMD+="$(mode_args $1 server 127.0.0.1) "
    CMD+="nesquic/$1 "
    CMD+="server -j ${EXP_NAME} --cert /workspace/res/pem/cert.pem --key /workspace/res/pem/key.pem 0.0.0.0:4433  -L nesquic_run:${NQ_RUN_LABEL} -L nesquic_invocation:${NQ_INVOCATION} "
    # Without InfluxDB, libnesquic.so prints its metrics instead.
    if [[ ${EXP_MODE} == qlog ]]; then
        CMD+="> /dev/null "
    fi
    CMD+="&"

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

    cpu_governor "schedutil"

    echo -e "${COLOR_YELLOW}Resetting CPU isolation${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_ALL}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_ALL}
}

function setup {
    # Created here so the frontend's bind mount is not owned by root.
    mkdir -p ${RES_DIR}/qlog
    docker compose -f ${WORKSPACE}/docker/service.yml up -d --build frontend

    kill_nesquic KILL
    may_fail sudo ip link del ${VETH_MM}

    echo -e "${COLOR_YELLOW}Setting up firewall${COLOR_OFF}"
    sudo ufw allow from 10.0.0.0/24 to any port 8086
    sudo ufw allow from 10.0.0.0/24 to any port 4433

    cpu_governor "performance"

    echo -e "${COLOR_YELLOW}Isolating CPUs${COLOR_OFF}"
    sudo systemctl set-property --runtime user.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime system.slice AllowedCPUs=${CPU_SYSTEM}
    sudo systemctl set-property --runtime init.scope AllowedCPUs=${CPU_SYSTEM}
}

# Deletes earlier results of this library, experiment and run label, so that
# repeated runs are not merged.
function clear_experiment {
    local predicate="library=\\\"$1\\\" AND job=\\\"${EXP_NAME}\\\" AND nesquic_run=\\\"${NQ_RUN_LABEL}\\\""

    curl -sS --fail-with-body --retry 5 --retry-connrefused -X POST \
        "http://127.0.0.1:8086/api/v2/delete?org=${INFLUX_ORG:-nesquic}&bucket=${INFLUX_BUCKET:-nesquic}" \
        -H "Authorization: Token ${INFLUX_TOKEN:-nesquic-token}" \
        -H "Content-Type: application/json" \
        -d "{\"start\":\"1970-01-01T00:00:00Z\",\"stop\":\"2100-01-01T00:00:00Z\",\"predicate\":\"${predicate}\"}" && return

    echo -e "\n${COLOR_RED}Could not delete earlier results of ${EXP_NAME}${COLOR_OFF}"
    teardown
}

function run_experiment {
    clear_experiment $1

    # detached: without libnesquic.so; only the client's own report is printed.
    # attached: collects the metrics.
    # qlog: only writes qlog traces, which slows down the monitored library.
    for EXP_MODE in detached attached qlog; do
        # Every repetition overwrites the same qlog trace.
        local reps=${NQ_REPS}
        if [[ ${EXP_MODE} == qlog ]]; then
            reps=1
        fi

        for ((rep = 1; rep <= reps; rep++)); do
            echo -e "run ${EXP_NAME} (${EXP_MODE}, ${rep}/${reps})... "

            run_server $1
            wait_for_launch
            run_client $1

            # kill server and give it time to upload its metrics
            kill_nesquic
            wait_for_term
        done
    done

    echo -e "${COLOR_GREEN}ok${COLOR_OFF}"
}

function run_library_experiments {
    echo -e "${COLOR_YELLOW}Benchmarking $1${COLOR_OFF}"

    local config
    for config in "${EXPERIMENTS[@]}"; do
        eval "${config}"
        run_experiment $1
    done

    echo -e "${COLOR_GREEN}Done${COLOR_OFF}"
}

setup
trap teardown EXIT
trap 'exit 1' INT TERM

if [ "$#" -eq 0 ]; then
    LIBS=(${NQ_LIBS})
else
    LIBS=("$@")
fi

for LIB in "${LIBS[@]}"; do
    ${WORKSPACE}/script/build.sh ${LIB}
    run_library_experiments ${LIB}
    ${WORKSPACE}/script/qlog.sh ${LIB} > /dev/null \
        || echo -e "${COLOR_RED}Could not render the qlog traces of ${LIB}${COLOR_OFF}"
done
