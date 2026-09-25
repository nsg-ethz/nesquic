//! Hooks for the libc I/O calls QUIC libraries use to move datagrams.
//!
//! Replaces the eBPF syscall tracepoints of earlier versions. Unlike those,
//! only calls on UDP sockets are counted (not e.g. stdout or the metrics
//! upload), and the volume is the number of bytes the call actually
//! transferred rather than the buffer size it was passed.

use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use libc::{c_int, c_uint, c_void, iovec, mmsghdr, msghdr, size_t, sockaddr, socklen_t, ssize_t};

use super::metrics::{Syscall, METRICS};

const UNKNOWN: u8 = 0;
const UDP: u8 = 1;
const OTHER: u8 = 2;

/// Per-fd cache of whether it is a UDP socket, so the `getsockopt` lookup
/// runs once per fd rather than once per call. Reset whenever an fd number
/// may be reused (`close`, `socket`, `dup2`/`dup3`).
static FD_KIND: [AtomicU8; 1 << 16] = [const { AtomicU8::new(UNKNOWN) }; 1 << 16];

fn is_udp(fd: c_int) -> bool {
    if !super::enabled() {
        return false;
    }

    let slot = usize::try_from(fd).ok().and_then(|fd| FD_KIND.get(fd));
    if let Some(slot) = slot {
        match slot.load(Relaxed) {
            UDP => return true,
            OTHER => return false,
            _ => {}
        }
    }

    let kind = if query_is_udp(fd) { UDP } else { OTHER };
    if let Some(slot) = slot {
        slot.store(kind, Relaxed);
    }
    kind == UDP
}

fn query_is_udp(fd: c_int) -> bool {
    // SAFETY: getsockopt only writes `len` bytes to `proto`; errno is
    // restored so the hooked call's caller never observes our lookup.
    unsafe {
        let errno = *libc::__errno_location();
        let mut proto: c_int = 0;
        let mut len = std::mem::size_of::<c_int>() as socklen_t;
        let ok = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PROTOCOL,
            &mut proto as *mut c_int as *mut c_void,
            &mut len,
        ) == 0;
        *libc::__errno_location() = errno;
        ok && proto == libc::IPPROTO_UDP
    }
}

fn forget(fd: c_int) {
    if let Some(slot) = usize::try_from(fd).ok().and_then(|fd| FD_KIND.get(fd)) {
        slot.store(UNKNOWN, Relaxed);
    }
}

fn record(syscall: Syscall, ret: isize) {
    METRICS.record_io(syscall, ret.max(0) as u64);
}

/// Records an `sendmmsg`/`recvmmsg` call that returned `ret` messages.
unsafe fn record_mmsg(syscall: Syscall, msgvec: *const mmsghdr, ret: c_int) {
    let n = ret.max(0) as usize;
    let bytes: u64 = if n == 0 {
        0
    } else {
        std::slice::from_raw_parts(msgvec, n)
            .iter()
            .map(|m| m.msg_len as u64)
            .sum()
    };
    METRICS.record_io(syscall, bytes);
}

redhook::hook! {
    unsafe fn write(fd: c_int, buf: *const c_void, count: size_t) -> ssize_t => hook_write {
        let udp = is_udp(fd);
        let ret = redhook::real!(write)(fd, buf, count);
        if udp { record(Syscall::Write, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn writev(fd: c_int, iov: *const iovec, iovcnt: c_int) -> ssize_t => hook_writev {
        let udp = is_udp(fd);
        let ret = redhook::real!(writev)(fd, iov, iovcnt);
        if udp { record(Syscall::Writev, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn send(fd: c_int, buf: *const c_void, len: size_t, flags: c_int) -> ssize_t => hook_send {
        let udp = is_udp(fd);
        let ret = redhook::real!(send)(fd, buf, len, flags);
        if udp { record(Syscall::Send, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn sendto(
        fd: c_int, buf: *const c_void, len: size_t, flags: c_int,
        addr: *const sockaddr, addrlen: socklen_t
    ) -> ssize_t => hook_sendto {
        let udp = is_udp(fd);
        let ret = redhook::real!(sendto)(fd, buf, len, flags, addr, addrlen);
        if udp { record(Syscall::Sendto, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn sendmsg(fd: c_int, msg: *const msghdr, flags: c_int) -> ssize_t => hook_sendmsg {
        let udp = is_udp(fd);
        let ret = redhook::real!(sendmsg)(fd, msg, flags);
        if udp { record(Syscall::Sendmsg, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn sendmmsg(fd: c_int, msgvec: *mut mmsghdr, vlen: c_uint, flags: c_int) -> c_int => hook_sendmmsg {
        let udp = is_udp(fd);
        let ret = redhook::real!(sendmmsg)(fd, msgvec, vlen, flags);
        if udp { record_mmsg(Syscall::Sendmmsg, msgvec, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn read(fd: c_int, buf: *mut c_void, count: size_t) -> ssize_t => hook_read {
        let udp = is_udp(fd);
        let ret = redhook::real!(read)(fd, buf, count);
        if udp { record(Syscall::Read, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn readv(fd: c_int, iov: *const iovec, iovcnt: c_int) -> ssize_t => hook_readv {
        let udp = is_udp(fd);
        let ret = redhook::real!(readv)(fd, iov, iovcnt);
        if udp { record(Syscall::Readv, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn recv(fd: c_int, buf: *mut c_void, len: size_t, flags: c_int) -> ssize_t => hook_recv {
        let udp = is_udp(fd);
        let ret = redhook::real!(recv)(fd, buf, len, flags);
        if udp { record(Syscall::Recv, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn recvfrom(
        fd: c_int, buf: *mut c_void, len: size_t, flags: c_int,
        addr: *mut sockaddr, addrlen: *mut socklen_t
    ) -> ssize_t => hook_recvfrom {
        let udp = is_udp(fd);
        let ret = redhook::real!(recvfrom)(fd, buf, len, flags, addr, addrlen);
        if udp { record(Syscall::Recvfrom, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn recvmsg(fd: c_int, msg: *mut msghdr, flags: c_int) -> ssize_t => hook_recvmsg {
        let udp = is_udp(fd);
        let ret = redhook::real!(recvmsg)(fd, msg, flags);
        if udp { record(Syscall::Recvmsg, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn recvmmsg(
        fd: c_int, msgvec: *mut mmsghdr, vlen: c_uint, flags: c_int,
        timeout: *mut libc::timespec
    ) -> c_int => hook_recvmmsg {
        let udp = is_udp(fd);
        let ret = redhook::real!(recvmmsg)(fd, msgvec, vlen, flags, timeout);
        if udp { record_mmsg(Syscall::Recvmmsg, msgvec, ret) }
        ret
    }
}

redhook::hook! {
    unsafe fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int => hook_socket {
        let fd = redhook::real!(socket)(domain, ty, protocol);
        forget(fd);
        fd
    }
}

redhook::hook! {
    unsafe fn close(fd: c_int) -> c_int => hook_close {
        forget(fd);
        redhook::real!(close)(fd)
    }
}

redhook::hook! {
    unsafe fn dup2(oldfd: c_int, newfd: c_int) -> c_int => hook_dup2 {
        forget(newfd);
        redhook::real!(dup2)(oldfd, newfd)
    }
}

redhook::hook! {
    unsafe fn dup3(oldfd: c_int, newfd: c_int, flags: c_int) -> c_int => hook_dup3 {
        forget(newfd);
        redhook::real!(dup3)(oldfd, newfd, flags)
    }
}
