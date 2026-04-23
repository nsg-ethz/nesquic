#include "vmlinux.h"
#include "bpf_tracing.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_endian.h>

char LICENSE[] SEC("license") = "GPL";

volatile const u32 MONITORED_PID;
#define pid_guard(...) if ((bpf_get_current_pid_tgid() >> 32) != MONITORED_PID) return 0

const u16 EVENT_IO_SYSCALL_WRITE = 0;
const u16 EVENT_IO_SYSCALL_WRITEV = 1;
const u16 EVENT_IO_SYSCALL_SEND = 2;
const u16 EVENT_IO_SYSCALL_SENDTO = 3;
const u16 EVENT_IO_SYSCALL_SENDMSG = 4;
const u16 EVENT_IO_SYSCALL_SENDMMSG = 5;

const u16 EVENT_IO_SYSCALL_READ = 6;
const u16 EVENT_IO_SYSCALL_READV = 7;
const u16 EVENT_IO_SYSCALL_RECV = 8;
const u16 EVENT_IO_SYSCALL_RECVFROM = 9;
const u16 EVENT_IO_SYSCALL_RECVMSG = 10;
const u16 EVENT_IO_SYSCALL_RECVMMSG = 11;

struct ringbuf {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 5000);
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
        bpf_error("_submit_event failed to reserve %u bytes", len);
        bpf_ringbuf_discard_dynptr(&ptr, 0);
        return;
    }

    if (bpf_dynptr_write(&ptr, 0, event, len, 0) != 0) {
        bpf_error("_submit_event failed to write dynptr");
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
            bpf_error("count_iovec_len failed to read iov[%u]", i);
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
        bpf_error("_submit_event_user_msghdr failed to read msg");
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
            bpf_error("_submit_event_msghdr failed to read mmsg[%u]", i);
            return;
        }

        k += count_iovec_len(msg.msg_hdr.msg_iov, msg.msg_hdr.msg_iovlen);
    }

    _submit_event_io(syscall, k);
}

// struct trace_sys_enter_writev_args {
//    short common_type;
//    char common_flags;
//    char common_preempt_count;
//    int common_pid;
//    s32 syscall_nr;
//    unsigned long fd;
//    struct iovec *vec;
//    unsigned long vlen;
// };

SEC("tracepoint/syscalls/sys_enter_writev")
int writev(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    unsigned long fd = ctx->args[0];
    struct iovec *vec = (struct iovec *)ctx->args[1];
    unsigned long vlen = ctx->args[2];

    bpf_trace("writev(%lu, %p, %lu)", fd, vec, vlen);
    _submit_event_iovec(EVENT_IO_SYSCALL_WRITEV, vec, vlen);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_readv")
int readv(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    unsigned long fd = ctx->args[0];
    struct iovec *vec = (struct iovec *)ctx->args[1];
    unsigned long vlen = ctx->args[2];

    bpf_trace("readv(%lu, %p, %lu)", fd, vec, vlen);
    _submit_event_iovec(EVENT_IO_SYSCALL_READV, vec, vlen);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_write")
int write(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    unsigned int fd = ctx->args[0];
    const char *buf = (const char *)ctx->args[1];
    size_t count = ctx->args[2];

    bpf_trace("write(%u, %p, %lu)", fd, buf, count);
    _submit_event_io(EVENT_IO_SYSCALL_WRITE, count);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_read")
int read(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    unsigned int fd = ctx->args[0];
    char *buf = (char *)ctx->args[1];
    size_t count = ctx->args[2];

    bpf_trace("read(%u, %p, %lu)", fd, buf, count);
    _submit_event_io(EVENT_IO_SYSCALL_READ, count);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_recvmsg")
int recvmsg(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    int fd = ctx->args[0];
    struct user_msghdr *msg = (struct user_msghdr *)ctx->args[1];
    unsigned int flags = ctx->args[2];

    bpf_trace("recvmsg(%u, %p, %u)", fd, msg, flags);
    _submit_event_user_msghdr(EVENT_IO_SYSCALL_RECVMSG, msg);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_recvmmsg")
int recvmmsg(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    int fd = ctx->args[0];
    struct mmsghdr *mmsg = (struct mmsghdr *)ctx->args[1];
    unsigned int vlen = ctx->args[2];
    unsigned int flags = ctx->args[3];

    bpf_trace("recvmmsg(%u, %p, %u, %u)", fd, mmsg, vlen, flags);
    _submit_event_msghdr(EVENT_IO_SYSCALL_RECVMMSG, mmsg, vlen);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_recvfrom")
int recvfrom(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    int fd = ctx->args[0];
    void *buf = (void *)ctx->args[1];
    size_t size = ctx->args[2];
    unsigned int flags = ctx->args[3];
    struct sockaddr *addr = (struct sockaddr *)ctx->args[4];
    int *addr_len = (int *)ctx->args[5];

    bpf_trace("recvfrom(%u, %p, %lu, %u, %p, %p)", fd, buf, size, flags, addr, addr_len);
    _submit_event_io(EVENT_IO_SYSCALL_RECVFROM, size);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_sendto")
int sendto(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    int fd = ctx->args[0];
    void *buf = (void *)ctx->args[1];
    size_t len = ctx->args[2];
    unsigned int flags = ctx->args[3];
    struct sockaddr *addr = (struct sockaddr *)ctx->args[4];
    int addr_len = ctx->args[5];

    bpf_trace("sendto(%u, %p, %lu, %u, %p, %d)", fd, buf, len, flags, addr, addr_len);
    _submit_event_io(EVENT_IO_SYSCALL_SENDTO, len);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_sendmsg")
int sendmsg(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    int fd = ctx->args[0];
    struct user_msghdr *msg = (struct user_msghdr *)ctx->args[1];
    unsigned int flags = ctx->args[2];

    bpf_trace("sendmsg(%u, %p, %u)", fd, msg, flags);
    _submit_event_user_msghdr(EVENT_IO_SYSCALL_SENDMSG, msg);

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_sendmmsg")
int sendmmsg(struct trace_event_raw_sys_enter *ctx) {
    pid_guard();

    int fd = ctx->args[0];
    struct mmsghdr *mmsg = (struct mmsghdr *)ctx->args[1];
    unsigned int vlen = ctx->args[2];
    unsigned int flags = ctx->args[3];

    bpf_trace("sendmmsg(%u, %p, %u, %u)", fd, mmsg, vlen, flags);
    _submit_event_msghdr(EVENT_IO_SYSCALL_SENDMMSG, mmsg, vlen);

    return 0;
}
