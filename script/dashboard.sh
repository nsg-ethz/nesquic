WORKSPACE=$(dirname "$(readlink -f "$0")")/..

set -e

function generate_dashboard {
    OUT=${WORKSPACE}/docker/grafana/dashboard/$1.json
    uv run --project ${WORKSPACE} python ${WORKSPACE}/script/dashboard.py -o ${OUT} $1
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
generate_dashboard overview

docker compose -f ${WORKSPACE}/docker/backend.yml restart grafana
