#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_endian.h>
#include <sys/cdefs.h>

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

const u16 EVENT_IO_SYSCALL_WRITE = 1;
const u16 EVENT_IO_SYSCALL_READ = 2;
const u16 EVENT_IO_SYSCALL_WRITEV = 3;
const u16 EVENT_IO_SYSCALL_READV = 4;
const u16 EVENT_IO_SYSCALL_RECV = 5;
const u16 EVENT_IO_SYSCALL_RECVFROM = 6;
const u16 EVENT_IO_SYSCALL_RECVMSG = 7;
const u16 EVENT_IO_SYSCALL_RECVMMSG = 8;
const u16 EVENT_IO_SYSCALL_SEND = 9;
const u16 EVENT_IO_SYSCALL_SENDTO = 10;
const u16 EVENT_IO_SYSCALL_SENDMSG = 11;
const u16 EVENT_IO_SYSCALL_SENDMMSG = 12;

struct ringbuf {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1000);
} events SEC(".maps");

struct event_io {
    u16 syscall;
    u32 len;
};

// so that libbpf exports it
struct event_io noevent = {};

__always_inline void _submit_event(void *event, u32 len) {
    struct bpf_dynptr ptr;
    if (bpf_ringbuf_reserve_dynptr(&events, len, 0, &ptr) != 0) {
        bpf_err("ERROR: _submit_event failed to reserve dynptr");
        bpf_ringbuf_discard_dynptr(&ptr, 0);
        return;
    }

    if (bpf_dynptr_write(&ptr, 0, event, len, 0) != 0) {
        bpf_err("ERROR: _submit_event failed to write dynptr");
        bpf_ringbuf_discard_dynptr(&ptr, 0);
        return;
    }

    bpf_ringbuf_submit_dynptr(&ptr, 0);
}

__always_inline void _submit_event_io(u16 syscall, u32 len) {
    struct event_io ev = {
        .syscall = syscall,
        .len = len,
    };

    _submit_event(&ev, sizeof(struct event_io));
}

__always_inline u32 count_iovec_len(struct iovec *vec, u32 vlen) {
    u32 i = 0, k = 0;
    bpf_for(i, 0, vlen) {
        struct iovec iov;
        if (bpf_probe_read_user(&iov, sizeof(struct iovec), vec + i) < 0) {
            bpf_err("ERROR: count_iovec_len failed to read iov[%u]", i);
            return 0;
        }

        k += iov.iov_len;
    }

    return k;
}

__always_inline void _submit_event_iovec(u16 syscall, struct iovec *vec, u32 vlen) {
    u32 k = count_iovec_len(vec, vlen);
    _submit_event_io(syscall, k);
}

__always_inline void _submit_event_user_msghdr(u16 syscall, struct user_msghdr *msg_ptr) {
    u32 i = 0, k = 0;
    struct user_msghdr msg;
    if (bpf_probe_read_user(&msg, sizeof(struct user_msghdr), msg_ptr) < 0) {
        bpf_err("ERROR: _submit_event_user_msghdr failed to read msg");
        return;
    }

    bpf_for(i, 0, msg.msg_iovlen) {
        k += count_iovec_len(msg.msg_iov, msg.msg_iovlen);
    }

    _submit_event_io(syscall, k);
}

__always_inline void _submit_event_msghdr(u16 syscall, struct mmsghdr *mmsg, u32 vlen) {
    u32 i = 0, j = 0, k = 0;
    bpf_for(i, 0, vlen) {
        struct mmsghdr msg;
        if (bpf_probe_read_user(&msg, sizeof(struct mmsghdr), mmsg + i) < 0) {
            bpf_err("ERROR: _submit_event_msghdr failed to read mmsg[%u]", i);
            return;
        }

        k += count_iovec_len(msg.msg_hdr.msg_iov, msg.msg_hdr.msg_iovlen);
    }

    _submit_event_io(syscall, k);
}

SEC("kprobe/do_writev")
int BPF_KPROBE(do_writev, unsigned long fd, struct iovec *vec, unsigned long vlen, rwf_t flags) {
    pid_guard();
    bpf_log("do_writev(%lu, %p, %lu, %u)", fd, vec, vlen, flags);
    _submit_event_iovec(EVENT_IO_SYSCALL_WRITEV, vec, vlen);

    return 0;
}

SEC("kprobe/do_readv")
int BPF_KPROBE(do_readv, unsigned long fd, struct iovec* vec, unsigned long vlen) {
    pid_guard();
    bpf_log("do_readv(%lu, %p, %lu)", fd, vec, vlen);
    _submit_event_iovec(EVENT_IO_SYSCALL_READV, vec, vlen);

    return 0;
}

SEC("kprobe/ksys_write")
int BPF_KPROBE(ksys_write, int fd, char *buf, size_t len) {
    pid_guard();
    // bpf_log("ksys_write(%d, %p, %lu)", fd, buf, len);
    // _submit_event_io(EVENT_IO_SYSCALL_WRITE, len);

    return 0;
}

SEC("kprobe/ksys_read")
int BPF_KPROBE(ksys_read, int fd, char *buf, size_t len) {
    pid_guard();
    bpf_log("ksys_read(%d, %p, %lu)", fd, buf, len);
    _submit_event_io(EVENT_IO_SYSCALL_READ, len);

    return 0;
}

SEC("kprobe/__sys_recvmsg")
int BPF_KPROBE(__sys_recvmsg, int fd, struct user_msghdr *msg, unsigned int flags, bool forbid_cmsg_compat) {
    pid_guard();
    bpf_log("__sys_recvmsg(%u, %p, %u, %u)", fd, msg, flags, forbid_cmsg_compat);
    _submit_event_user_msghdr(EVENT_IO_SYSCALL_RECVMSG, msg);

    return 0;
}

SEC("kprobe/__sys_recvmmsg")
int BPF_KPROBE(__sys_recvmmsg, int fd, struct mmsghdr *mmsg, unsigned int vlen, unsigned int flags, struct timespec64 *timeout) {
    pid_guard();
    bpf_log("__sys_recvmmsg(%u, %p, %u, %u, %p)", fd, mmsg, vlen, flags, timeout);
    _submit_event_msghdr(EVENT_IO_SYSCALL_RECVMMSG, mmsg, vlen);

    return 0;
}

SEC("kprobe/__sys_recvfrom")
int BPF_KPROBE(__sys_recvfrom, int fd, void *buf, size_t size, unsigned int flags, struct sockaddr *addr, int *addr_len) {
    pid_guard();
    bpf_log("__sys_recvfrom(%u, %p, %lu, %u, %p, %p)", fd, buf, size, flags, addr, addr_len);
    _submit_event_io(EVENT_IO_SYSCALL_RECVFROM, size);

    return 0;
}

SEC("kprobe/__sys_sendto")
int BPF_KPROBE(__sys_sendto, int fd, void *buf, size_t len, unsigned int flags, struct sockaddr *addr,  int addr_len) {
    pid_guard();
    bpf_log("__sys_sendto(%u, %p, %lu, %u, %p, %p)", fd, buf, len, flags, addr, addr_len);
    _submit_event_io(EVENT_IO_SYSCALL_SENDTO, len);

    return 0;
}

SEC("kprobe/__sys_sendmsg")
int BPF_KPROBE(__sys_sendmsg, int fd, struct user_msghdr *msg, unsigned int flags, bool forbid_cmsg_compat) {
    pid_guard();
    bpf_log("__sys_sendmsg(%u, %p, %u, %u)", fd, msg, flags, forbid_cmsg_compat);
    _submit_event_user_msghdr(EVENT_IO_SYSCALL_SENDMSG, msg);

    return 0;
}

SEC("kprobe/__sys_sendmmsg")
int BPF_KPROBE(__sys_sendmmsg, int fd, struct mmsghdr *mmsg, unsigned int vlen, unsigned int flags, bool forbid_cmsg_compat) {
    pid_guard();
    bpf_log("__sys_sendmmsg(%u, %p, %u, %u, %u)", fd, mmsg, vlen, flags, forbid_cmsg_compat);
    _submit_event_msghdr(EVENT_IO_SYSCALL_SENDMMSG, mmsg, vlen);

    return 0;
}
