//! Hooks for the libc I/O calls QUIC libraries use to move datagrams.
//!
//! Only calls on UDP sockets are counted (not e.g. stdout or the metrics
//! upload), and the volume is the number of bytes the call actually
//! transferred rather than the buffer size it was passed.

use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use libc::{c_int, c_uint, c_void, iovec, mmsghdr, msghdr, size_t, sockaddr, socklen_t, ssize_t};

use super::metrics::{Syscall, METRICS};
use super::qlog;

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

/// The address `addr` of `len` bytes, if it is an IP socket address.
unsafe fn socket_addr(addr: *const sockaddr, len: socklen_t) -> Option<SocketAddr> {
    if addr.is_null() {
        return None;
    }
    let len = len as usize;
    match c_int::from((*addr).sa_family) {
        libc::AF_INET if len >= std::mem::size_of::<libc::sockaddr_in>() => {
            let a = &*(addr as *const libc::sockaddr_in);
            let ip = u32::from_be(a.sin_addr.s_addr).into();
            Some(SocketAddrV4::new(ip, u16::from_be(a.sin_port)).into())
        }
        libc::AF_INET6 if len >= std::mem::size_of::<libc::sockaddr_in6>() => {
            let a = &*(addr as *const libc::sockaddr_in6);
            let ip = a.sin6_addr.s6_addr.into();
            let port = u16::from_be(a.sin6_port);
            Some(SocketAddrV6::new(ip, port, a.sin6_flowinfo, a.sin6_scope_id).into())
        }
        _ => None,
    }
}

/// The first `n` bytes in the buffers `iov`.
unsafe fn iov_slices<'a>(iov: *const iovec, iovcnt: usize, mut n: usize) -> Vec<&'a [u8]> {
    if iov.is_null() {
        return Vec::new();
    }
    let mut bufs = Vec::new();
    for v in std::slice::from_raw_parts(iov, iovcnt) {
        let len = v.iov_len.min(n);
        if len == 0 || v.iov_base.is_null() {
            break;
        }
        bufs.push(std::slice::from_raw_parts(v.iov_base as *const u8, len));
        n -= len;
    }
    bufs
}

/// The segment size of a GSO send (`UDP_SEGMENT`), which puts several
/// datagrams into one buffer.
unsafe fn gso_size(msg: &msghdr) -> Option<usize> {
    let mut cmsg = libc::CMSG_FIRSTHDR(msg);
    while !cmsg.is_null() {
        if (*cmsg).cmsg_level == libc::SOL_UDP && (*cmsg).cmsg_type == libc::UDP_SEGMENT {
            let size = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg) as *const u16);
            return (size > 0).then_some(size as usize);
        }
        cmsg = libc::CMSG_NXTHDR(msg, cmsg);
    }
    None
}

/// Tells [`qlog`] about the datagrams `bufs` that went through the UDP
/// socket `fd` to or from `peer`.
fn datagrams(fd: c_int, peer: Option<SocketAddr>, bufs: &[&[u8]], sent: bool) {
    if qlog::enabled() {
        qlog::on_datagrams(fd, peer, bufs, sent);
    }
}

unsafe fn buf_datagram(
    fd: c_int,
    buf: *const c_void,
    ret: isize,
    peer: Option<SocketAddr>,
    sent: bool,
) {
    if ret > 0 && !buf.is_null() {
        datagrams(
            fd,
            peer,
            &[std::slice::from_raw_parts(buf as *const u8, ret as usize)],
            sent,
        );
    }
}

unsafe fn msg_datagram(fd: c_int, msg: *const msghdr, n: isize, sent: bool) {
    if n <= 0 || msg.is_null() || !qlog::enabled() {
        return;
    }
    let msg = &*msg;
    let peer = socket_addr(msg.msg_name as *const sockaddr, msg.msg_namelen);
    let mut bufs = iov_slices(msg.msg_iov, msg.msg_iovlen, n as usize);
    if let (true, Some(size), [buf]) = (sent, gso_size(msg), &bufs[..]) {
        bufs = buf.chunks(size).collect();
    }
    datagrams(fd, peer, &bufs, sent);
}

unsafe fn mmsg_datagrams(fd: c_int, msgvec: *const mmsghdr, ret: c_int, sent: bool) {
    if ret <= 0 || msgvec.is_null() {
        return;
    }
    for m in std::slice::from_raw_parts(msgvec, ret as usize) {
        msg_datagram(fd, &m.msg_hdr, m.msg_len as isize, sent);
    }
}

unsafe fn iov_datagram(fd: c_int, iov: *const iovec, iovcnt: c_int, ret: isize, sent: bool) {
    if ret > 0 && qlog::enabled() {
        datagrams(
            fd,
            None,
            &iov_slices(iov, iovcnt.max(0) as usize, ret as usize),
            sent,
        );
    }
}

redhook::hook! {
    unsafe fn write(fd: c_int, buf: *const c_void, count: size_t) -> ssize_t => hook_write {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(write)(fd, buf, count));
        if udp {
            record(Syscall::Write, ret);
            buf_datagram(fd, buf, ret, None, true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn writev(fd: c_int, iov: *const iovec, iovcnt: c_int) -> ssize_t => hook_writev {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(writev)(fd, iov, iovcnt));
        if udp {
            record(Syscall::Writev, ret);
            iov_datagram(fd, iov, iovcnt, ret, true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn send(fd: c_int, buf: *const c_void, len: size_t, flags: c_int) -> ssize_t => hook_send {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(send)(fd, buf, len, flags));
        if udp {
            record(Syscall::Send, ret);
            buf_datagram(fd, buf, ret, None, true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn sendto(
        fd: c_int, buf: *const c_void, len: size_t, flags: c_int,
        addr: *const sockaddr, addrlen: socklen_t
    ) -> ssize_t => hook_sendto {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(sendto)(fd, buf, len, flags, addr, addrlen));
        if udp {
            record(Syscall::Sendto, ret);
            buf_datagram(fd, buf, ret, socket_addr(addr, addrlen), true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn sendmsg(fd: c_int, msg: *const msghdr, flags: c_int) -> ssize_t => hook_sendmsg {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(sendmsg)(fd, msg, flags));
        if udp {
            record(Syscall::Sendmsg, ret);
            msg_datagram(fd, msg, ret, true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn sendmmsg(fd: c_int, msgvec: *mut mmsghdr, vlen: c_uint, flags: c_int) -> c_int => hook_sendmmsg {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(sendmmsg)(fd, msgvec, vlen, flags));
        if udp {
            record_mmsg(Syscall::Sendmmsg, msgvec, ret);
            mmsg_datagrams(fd, msgvec, ret, true);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn read(fd: c_int, buf: *mut c_void, count: size_t) -> ssize_t => hook_read {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(read)(fd, buf, count));
        if udp {
            record(Syscall::Read, ret);
            buf_datagram(fd, buf, ret, None, false);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn readv(fd: c_int, iov: *const iovec, iovcnt: c_int) -> ssize_t => hook_readv {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(readv)(fd, iov, iovcnt));
        if udp {
            record(Syscall::Readv, ret);
            iov_datagram(fd, iov, iovcnt, ret, false);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn recv(fd: c_int, buf: *mut c_void, len: size_t, flags: c_int) -> ssize_t => hook_recv {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(recv)(fd, buf, len, flags));
        if udp {
            record(Syscall::Recv, ret);
            buf_datagram(fd, buf, ret, None, false);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn recvfrom(
        fd: c_int, buf: *mut c_void, len: size_t, flags: c_int,
        addr: *mut sockaddr, addrlen: *mut socklen_t
    ) -> ssize_t => hook_recvfrom {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(recvfrom)(fd, buf, len, flags, addr, addrlen));
        if udp {
            record(Syscall::Recvfrom, ret);
            let peer = if addrlen.is_null() { None } else { socket_addr(addr, *addrlen) };
            buf_datagram(fd, buf, ret, peer, false);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn recvmsg(fd: c_int, msg: *mut msghdr, flags: c_int) -> ssize_t => hook_recvmsg {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(recvmsg)(fd, msg, flags));
        if udp {
            record(Syscall::Recvmsg, ret);
            msg_datagram(fd, msg, ret, false);
        }
        ret
    }
}

redhook::hook! {
    unsafe fn recvmmsg(
        fd: c_int, msgvec: *mut mmsghdr, vlen: c_uint, flags: c_int,
        timeout: *mut libc::timespec
    ) -> c_int => hook_recvmmsg {
        let udp = is_udp(fd);
        let ret = METRICS.time_io(udp, || redhook::real!(recvmmsg)(fd, msgvec, vlen, flags, timeout));
        if udp {
            record_mmsg(Syscall::Recvmmsg, msgvec, ret);
            mmsg_datagrams(fd, msgvec, ret, false);
        }
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
