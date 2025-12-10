WORKSPACE=$(dirname "$(readlink -f "$0")")/..

set -e

function generate_dashboard {
    OUT=${WORKSPACE}/docker/grafana/dashboard/$1.json
    LIBRARY=$1 EXPERIMENTS=${WORKSPACE}/res/experiments.yaml uv tool run --with pyyaml --from git+https://github.com/lbrndnr/grafanalib@main generate-dashboard -o ${OUT} ${WORKSPACE}/script/main.dashboard.py
    chmod o+r ${OUT}
}

if [ "$#" -eq 0 ]; then
    LIBS=(${NQ_LIBS})
else
    LIBS=("$@")
fi

for LIB in "${LIBS[@]}"; do
    generate_dashboard ${LIB}
done

docker compose -f ${WORKSPACE}/docker/backend.yml restart grafana
