WORKSPACE=$(dirname "$(readlink -f "$0")")/..

set -e

LIBRARY=quinn ${WORKSPACE}/script/main.dashboard.py
docker compose -f ${WORKSPACE}/docker/backend.yml restart grafana
