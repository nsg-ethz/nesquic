CLIENT_BIN="target/release/client"
SERVER_BIN="target/release/server"
# PERF_CMD="sudo perf record -F 99 -g -a -e syscalls:sys_enter_*"
PERF_CMD="sudo perf record -F 99 -g -a"

echo Start server in background
${SERVER_BIN} --cert res/ca/cert.der --key res/ca/key.der --unencrypted &> /dev/null &

echo Start client
${PERF_CMD} ${CLIENT_BIN} --cert res/ca/cert.der --unencrypted https://localhost:4433/20Gbit
kill %1