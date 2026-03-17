# Better QUIC implementations with <img width="150" alt="nesquic" src="https://github.com/user-attachments/assets/5f5ab452-e3a6-41f3-a209-5ba2308f6188" />

Nesquic is a testing infrastructure for QUIC libraries. It leverages eBPF to monitor library-internal QUIC components, like for example cryptography, or I/O. This allows the user to compare different design choices, find bottlenecks and improve the performance of their QUIC library.

Nesquic provides multiple QUIC client and server implementations (see [status](#status) for supported libraries), as well as a set of testing regimes to evaluate them. It leverages [Mahimahi](https://github.com/ravinet/mahimahi) to emulate realistic network conditions during the test, and eBPF to collect library-internal metrics. Nesquic then generates a Grafana dashboard with the results.

## Getting Started

First, install [Mahimahi](https://github.com/ravinet/mahimahi), along with some other Nesquic dependencies:
```
sudo apt install -y apache2-bin apache2-dev cmake libcairo2-dev libpango1.0-dev libnuma-dev libxcb-present-dev dnsmasq-base protobuf-compiler ssl-cert libssl-dev binutils-dev libpcap-dev
git clone https://github.com/ravinet/mahimahi
cd mahimahi
./autogen.sh
./configure
make
sudo make install
cd ..
rm -rf mahimahi
cargo install --locked uv
```

Next, install [libbpf](https://github.com/libbpf/libbpf) and [bpftool](https://github.com/libbpf/bpftool) from source.

Then, generate a new vmlinux file as follows:
```
bpftool btf dump file /sys/kernel/btf/vmlinux format c > include/vmlinux.h
```

For Neqo, the following additional dependencies are needed: `libnss3`. While the distributed versions are not up to date, we will use a static version in `static_dependencies/neqo_dependencies`.
In order to use it, you need to set the following environment variables: 
```shell
export LD_LIBRARY_PATH=echo $(pwd)/static_dependencies/neqo_dependencies/dist/Release/lib
export NSS_DIR=$(pwd)/static_dependencies/neqo_dependencies/nss
export NSS_PREBUILT=1
```

In order to create those libraries, the following process which follows Neqo's README was used:
```shell
apt install ninja-build mercurial
uv tool install gyp-next
mkdir ~/neqo_dependencies
cd neqo_dependencies
hg clone https://hg-edge.mozilla.org/projects/nss
hg clone https://hg-edge.mozilla.org/projects/nspr
export NSS_DIR=$HOME/neqo_dependencies/nss
cd ~/nesquic
cargo build

# after first successful build, the source files for nss and nspr are not needed anymore
cd ~/nesquic
mkdir -p ~/nesquic/static_dependencies/neqo_dependencies
cp -r ~/neqo_depencencies/dist ~/nesquic/static_dependencies/neqo_dependencies/
mdkir -p ~/nesquic/static_dependencies/neqo_dependencies/nss
mdkir -p ~/nesquic/static_dependencies/neqo_dependencies/nspr
touch ~/nesquic/static_dependencies/neqo_dependencies/nss/.gitkeep
touch ~/nesquic/static_dependencies/neqo_dependencies/nspr/.gitkeep
export LD_LIBRARY_PATH=echo $(pwd)/static_dependencies/neqo_dependencies/dist/Release/lib
export NSS_DIR=$(pwd)/static_dependencies/neqo_dependencies/nss
export NSS_PREBUILT=1
```

Now you can run a performance test as follows:
```
# sanity check that all client and server implementations work
cargo test
# start the metric collection services (prometheus and grafana)
docker compose -f docker/backend.yml up -d
# run the test scenarios for a given library
script/run.sh quinn quiche
```

This starts the Grafana dashboard and executes a performance test. The dashboard is hosted at `http://localhost:3000`

During development, it might be practical to use the following environment variables:
```
export NQ_LIBS="quinn quiche"
export PR_PUSH_GATEWAY="http://localhost:9091"
```

To reset the Grafana dashboard, simply remove the `nesquic_grafana_data` volume:
```
docker compose -f docker/backend.yml down
docker volume rm nesquic_grafana_data
```

## Status

| Library          | Status                                  |
|------------------|-----------------------------------------|
| [Quinn](https://github.com/quinn-rs/quinn)        | ✅     |
| [Quiche](https://github.com/cloudflare/quiche)    | ✅     |
| [MsQuic](https://github.com/microsoft/msquic)     | WIP    |
| [Neqo](https://github.com/mozilla/neqo)           | WIP    |
