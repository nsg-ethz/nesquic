LIB="${1:-}"
if [[ -z "${LIB}" ]]; then
    echo "usage: $0 <library>" >&2
    exit 2
fi

WORKSPACE=$(dirname "$(readlink -f "$0")")/..

docker build -f ${WORKSPACE}/docker/Dockerfile.rust -t nesquic/rust ${WORKSPACE}
docker build -f ${WORKSPACE}/docker/Dockerfile.mahimahi -t nesquic/mahimahi ${WORKSPACE}

docker build -f ${WORKSPACE}/docker/Dockerfile.${LIB} -t nesquic/${LIB} ${WORKSPACE}
