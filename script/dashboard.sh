WORKSPACE=$(dirname "$(readlink -f "$0")")/..

set -e

function generate_dashboard {
    OUT=${WORKSPACE}/docker/grafana/dashboard/$1.json
    LIBRARY=$1 uv tool run --from git+https://github.com/lbrndnr/grafanalib@main generate-dashboard -o ${OUT} ${WORKSPACE}/script/main.dashboard.py
    chmod o+r ${OUT}
}

generate_dashboard quinn
generate_dashboard quiche

docker compose -f ${WORKSPACE}/docker/backend.yml restart grafana
