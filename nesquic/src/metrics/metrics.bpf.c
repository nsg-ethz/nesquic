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
#define pid_guard(...) if ((bpf_get_current_pid_tgid() >> 32) != MONITORED_PID) return 0

#define define_io_metric(NAME) u32 NAME##_num_calls = 0; u32 NAME##_data_size = 0

__always_inline void _sync_fetch_and_add_iovec(struct iovec *vec, unsigned long vlen, u32 *call, u32 *data_size) {
    u32 sum = 0;
    u32 i = 0;
    bpf_for(i, 0, vlen) {
        struct iovec iov;
        if (bpf_probe_read_user(&iov, sizeof(struct iovec), vec + i) < 0) {
            bpf_err("ERROR: sync_fetch_iovec failed to read iov[%u]", i);
            return;
        }

        sum += iov.iov_len;
    }

    __sync_fetch_and_add(call, 1);
    __sync_fetch_and_add(data_size, sum);
}

define_io_metric(do_writev);
SEC("kprobe/do_writev")
int BPF_KPROBE(do_writev, unsigned long fd, struct iovec *vec, unsigned long vlen, rwf_t flags) {
    pid_guard();
    bpf_log("do_writev(%lu, %p, %lu, %u)", fd, vec, vlen, flags);
    _sync_fetch_and_add_iovec(vec, vlen, &do_writev_num_calls, &do_writev_data_size);

    return 0;
}

define_io_metric(do_readv);
SEC("kprobe/do_readv")
int BPF_KPROBE(do_readv, unsigned long fd, struct iovec* vec, unsigned long vlen) {
    pid_guard();
    bpf_log("do_readv(%lu, %p, %lu)", fd, vec, vlen);
    _sync_fetch_and_add_iovec(vec, vlen, &do_readv_num_calls, &do_readv_data_size);

    return 0;
}

define_io_metric(ksys_write);
SEC("kprobe/ksys_write")
int BPF_KPROBE(ksys_write, int fd, char *buf, size_t len) {
    pid_guard();
    bpf_log("ksys_write(%d, %p, %lu)", fd, buf, len);
    __sync_fetch_and_add(&ksys_write_num_calls, 1);
    __sync_fetch_and_add(&ksys_write_data_size, len);

    return 0;
}

define_io_metric(ksys_read);
SEC("kprobe/ksys_read")
int BPF_KPROBE(ksys_read, int fd, char *buf, size_t len) {
    pid_guard();
    bpf_log("ksys_read(%d, %p, %lu)", fd, buf, len);
    __sync_fetch_and_add(&ksys_read_num_calls, 1);
    __sync_fetch_and_add(&ksys_read_data_size, len);

    return 0;
}

SEC("kprobe/__sys_recvmmsg")
int BPF_KPROBE(__sys_recvmmsg, int fd, struct mmsghdr *mmsg, unsigned int vlen, unsigned int flags, struct timespec64 *timeout) {
    pid_guard();
    bpf_log("__sys_recvmmsg(%u, %p, %u, %u, %p)", fd, mmsg, vlen, flags, timeout);

    return 0;
}

define_io_metric(__sys_recvfrom);
SEC("kprobe/__sys_recvfrom")
int BPF_KPROBE(__sys_recvfrom, int fd, void *buf, size_t size, unsigned int flags, struct sockaddr *addr, int *addr_len) {
    pid_guard();
    bpf_log("__sys_recvfrom(%u, %p, %lu, %u, %p, %p)", fd, buf, size, flags, addr, addr_len);
    __sync_fetch_and_add(&__sys_recvfrom_num_calls, 1);
    __sync_fetch_and_add(&__sys_recvfrom_data_size, size);

    return 0;
}

define_io_metric(__sys_sendto);
SEC("kprobe/__sys_sendto")
int BPF_KPROBE(__sys_sendto, int fd, void *buf, size_t len, unsigned int flags, struct sockaddr *addr,  int addr_len) {
    pid_guard();
    bpf_log("__sys_sendto(%u, %p, %lu, %u, %p, %p)", fd, buf, len, flags, addr, addr_len);
    __sync_fetch_and_add(&__sys_sendto_num_calls, 1);
    __sync_fetch_and_add(&__sys_sendto_data_size, len);

    return 0;
}

define_io_metric(__sys_sendmsg);
SEC("kprobe/__sys_sendmsg")
int BPF_KPROBE(__sys_sendmsg, int fd, struct user_msghdr *msg, unsigned int flags, bool forbid_cmsg_compat) {
    pid_guard();
    bpf_log("__sys_sendmsg(%u, %p, %u, %u)", fd, msg, flags, forbid_cmsg_compat);

    return 0;
}

SEC("kprobe/__sys_sendmmsg")
int BPF_KPROBE(__sys_sendmmsg, int fd, struct mmsghdr *mmsg, unsigned int vlen, unsigned int flags, bool forbid_cmsg_compat) {
    pid_guard();
    bpf_log("__sys_sendmmsg(%u, %p, %u, %u, %u)", fd, mmsg, vlen, flags, forbid_cmsg_compat);

    return 0;
}
