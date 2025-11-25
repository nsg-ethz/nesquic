#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_endian.h>

char LICENSE[] SEC("license") = "GPL";

#if LOG_LEVEL == 1
    #define bpf_log(...) (0)
    #define bpf_err(...) bpf_printk(__VA_ARGS__)
#elif LOG_LEVEL == 2
    #define bpf_log(...) bpf_printk(__VA_ARGS__)
    #define bpf_err(...) bpf_printk(__VA_ARGS__)
#else
    #define bpf_log(...) (0)
    #define bpf_err(...) (0)
#endif

volatile const u32 MONITORED_PID;

int do_writev_num_calls = 0;

SEC("kprobe/do_writev")
int BPF_KPROBE(do_writev, unsigned long fd, const struct iovec *vec, unsigned long vlen, rwf_t flags) {
    bpf_log("do_writev(%lu, %p, %lu, %u)", fd, vec, vlen, flags);

    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid;
    // u32 tgid = pid_tgid >> 32;

    if (pid != MONITORED_PID) return 0;

    do_writev_num_calls++;

    u32 i = 0;
    bpf_for(i, 0, vlen) {
        struct iovec iov;
        if (bpf_probe_read_user(&iov, sizeof(struct iovec), vec + i) < 0) {
            bpf_err("do_writev: failed to read user memory");
            return 0;
        }

        __kernel_size_t iov_len = iov.iov_len;
        if (iov_len > 2048) iov_len = 2048;

    }

    return 0;
}
