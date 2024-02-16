
CLIENT_BIN="target/release/$1-client"
SERVER_BIN="target/release/$1-server"
PERF_CMD="sudo perf record -F 99 -g -a -e syscalls:sys_enter_*"
# PERF_CMD="sudo perf record -F 997 --call-graph dwarf,16384 -g"

echo Start server in background
${SERVER_BIN} --cert res/pem/cert.pem --key res/pem/key.pem &

sleep 0.5

echo Start client
RUST_LOG=info ${CLIENT_BIN} --cert res/pem/cert.pem --blob 200Mbit --reps 20 https://localhost:4433
kill %1