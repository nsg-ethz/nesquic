LIB="${1:-}"
if [[ -z "${LIB}" ]]; then
    echo "usage: $0 <library>" >&2
    exit 2
fi

docker build -f docker/Dockerfile.rust -t nesquic/rust .
docker build -f docker/Dockerfile.mahimahi -t nesquic/mahimahi .

docker build -f docker/Dockerfile.${LIB} -t nesquic/${LIB} .
