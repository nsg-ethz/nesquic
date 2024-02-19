
CLIENT_BIN="target/release/$1-client"
SERVER_BIN="target/release/$1-server"
PERF_CMD="sudo perf record --stat -F 99 -g -a -e syscalls:sys_enter_write,syscalls:sys_enter_sendmsg,syscalls:sys_enter_sendmmsg"
# PERF_CMD="sudo perf record -F 997 --call-graph dwarf,16384 -g"
STRACE_CMD="strace -c --output=$1.strace"

echo Start server in background
${SERVER_BIN} --cert res/pem/cert.pem --key res/pem/key.pem --unencrypted &

sleep 0.5

echo Start client
${STRACE_CMD} taskset 128 ${CLIENT_BIN} --cert res/pem/cert.pem --blob 200Mbit --reps 20 --unencrypted https://localhost:4433
kill %1