package main

/*
#include <errno.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int nq_socket(int family) {
	int fd = socket(family, SOCK_DGRAM, 0);
	if (fd < 0) return -errno;
	// Match the buffer size quic-go requests for its own sockets.
	int size = 7 << 20;
	setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &size, sizeof(size));
	setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &size, sizeof(size));
	return fd;
}

static int nq_fill(struct sockaddr_storage *ss, const uint8_t *ip, int iplen, uint16_t port) {
	memset(ss, 0, sizeof(*ss));
	if (iplen == 4) {
		struct sockaddr_in *sin = (struct sockaddr_in *)ss;
		sin->sin_family = AF_INET;
		sin->sin_port = htons(port);
		memcpy(&sin->sin_addr, ip, 4);
		return sizeof(*sin);
	}
	struct sockaddr_in6 *sin6 = (struct sockaddr_in6 *)ss;
	sin6->sin6_family = AF_INET6;
	sin6->sin6_port = htons(port);
	memcpy(&sin6->sin6_addr, ip, 16);
	return sizeof(*sin6);
}

static int nq_setbuf(int fd, int opt, int size) {
	return setsockopt(fd, SOL_SOCKET, opt, &size, sizeof(size)) == 0 ? 0 : -errno;
}

static int nq_bind(int fd, const uint8_t *ip, int iplen, uint16_t port) {
	struct sockaddr_storage ss;
	int len = nq_fill(&ss, ip, iplen, port);
	return bind(fd, (struct sockaddr *)&ss, len) == 0 ? 0 : -errno;
}

static int nq_local_port(int fd) {
	struct sockaddr_storage ss;
	socklen_t len = sizeof(ss);
	if (getsockname(fd, (struct sockaddr *)&ss, &len) != 0) return -errno;
	if (ss.ss_family == AF_INET) return ntohs(((struct sockaddr_in *)&ss)->sin_port);
	return ntohs(((struct sockaddr_in6 *)&ss)->sin6_port);
}

// Waits up to timeout_ms for the socket to become readable: 1 if readable,
// 0 on timeout, -errno on error.
static int nq_wait(int fd, int timeout_ms) {
	struct pollfd pfd = {.fd = fd, .events = POLLIN};
	int n = poll(&pfd, 1, timeout_ms);
	return n < 0 ? -errno : n;
}

static ssize_t nq_recvfrom(int fd, void *buf, size_t len, uint8_t *ip, int *iplen,
                           uint16_t *port) {
	struct sockaddr_storage ss;
	socklen_t sslen = sizeof(ss);
	ssize_t n = recvfrom(fd, buf, len, MSG_DONTWAIT, (struct sockaddr *)&ss, &sslen);
	if (n < 0) return -errno;
	if (ss.ss_family == AF_INET) {
		struct sockaddr_in *sin = (struct sockaddr_in *)&ss;
		memcpy(ip, &sin->sin_addr, 4);
		*iplen = 4;
		*port = ntohs(sin->sin_port);
	} else {
		struct sockaddr_in6 *sin6 = (struct sockaddr_in6 *)&ss;
		memcpy(ip, &sin6->sin6_addr, 16);
		*iplen = 16;
		*port = ntohs(sin6->sin6_port);
	}
	return n;
}

static ssize_t nq_sendto(int fd, const void *buf, size_t len, const uint8_t *ip, int iplen,
                         uint16_t port) {
	struct sockaddr_storage ss;
	int sslen = nq_fill(&ss, ip, iplen, port);
	ssize_t n;
	do {
		n = sendto(fd, buf, len, 0, (struct sockaddr *)&ss, sslen);
	} while (n < 0 && errno == EINTR);
	return n < 0 ? -errno : n;
}
*/
import "C"

import (
	"errors"
	"net"
	"os"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
	"unsafe"
)

// libcConn is a UDP net.PacketConn whose I/O goes through libc's recvfrom
// and sendto. Go's net package issues raw syscalls, which the LD_PRELOAD
// monitor (libnesquic.so) cannot see; routing I/O through libc makes the
// nesquic I/O and throughput metrics work for quic-go.
//
// It deliberately does not implement quic-go's OOBCapablePacketConn, so
// quic-go falls back to one datagram per call (no GSO/GRO, no ECN).
type libcConn struct {
	fd     C.int
	local  *net.UDPAddr
	closed atomic.Bool

	mu           sync.Mutex
	readDeadline time.Time
}

var _ net.PacketConn = (*libcConn)(nil)

// pollInterval bounds how long a blocked read takes to notice Close or a
// new deadline.
const pollInterval = 50 * time.Millisecond

func errnoErr(op string, n C.long) error {
	return &net.OpError{Op: op, Net: "udp", Err: os.NewSyscallError(op, syscall.Errno(-n))}
}

func ipBytes(ip net.IP) []byte {
	if v4 := ip.To4(); v4 != nil {
		return v4
	}
	return ip.To16()
}

// listenLibc binds a UDP socket to addr.
func listenLibc(addr *net.UDPAddr) (*libcConn, error) {
	ip := ipBytes(addr.IP)
	if addr.IP == nil {
		ip = net.IPv4zero.To4()
	}
	family := C.int(C.AF_INET)
	if len(ip) == net.IPv6len {
		family = C.AF_INET6
	}

	fd := C.nq_socket(family)
	if fd < 0 {
		return nil, errnoErr("socket", C.long(fd))
	}
	if rv := C.nq_bind(fd, (*C.uint8_t)(unsafe.Pointer(&ip[0])), C.int(len(ip)), C.uint16_t(addr.Port)); rv < 0 {
		C.close(fd)
		return nil, errnoErr("bind", C.long(rv))
	}
	port := C.nq_local_port(fd)
	return &libcConn{fd: fd, local: &net.UDPAddr{IP: net.IP(ip), Port: int(port)}}, nil
}

func (c *libcConn) ReadFrom(b []byte) (int, net.Addr, error) {
	var ip [16]byte
	var iplen C.int
	var port C.uint16_t

	for {
		if c.closed.Load() {
			return 0, nil, net.ErrClosed
		}
		c.mu.Lock()
		deadline := c.readDeadline
		c.mu.Unlock()

		wait := pollInterval
		if !deadline.IsZero() {
			left := time.Until(deadline)
			if left <= 0 {
				return 0, nil, os.ErrDeadlineExceeded
			}
			wait = min(wait, left)
		}

		// Poll first so idle waiting does not show up as failed recvfrom calls.
		ready := C.nq_wait(c.fd, C.int((wait+time.Millisecond-1)/time.Millisecond))
		if ready < 0 {
			if syscall.Errno(-ready) == syscall.EINTR {
				continue
			}
			return 0, nil, errnoErr("poll", C.long(ready))
		}
		if ready == 0 {
			continue
		}

		n := C.nq_recvfrom(c.fd, unsafe.Pointer(&b[0]), C.size_t(len(b)),
			(*C.uint8_t)(unsafe.Pointer(&ip[0])), &iplen, &port)
		if n < 0 {
			if errno := syscall.Errno(-n); errno == syscall.EAGAIN || errno == syscall.EINTR {
				continue
			}
			return 0, nil, errnoErr("recvfrom", C.long(n))
		}
		addr := &net.UDPAddr{IP: append(net.IP(nil), ip[:iplen]...), Port: int(port)}
		return int(n), addr, nil
	}
}

func (c *libcConn) WriteTo(b []byte, addr net.Addr) (int, error) {
	if c.closed.Load() {
		return 0, net.ErrClosed
	}
	udp, ok := addr.(*net.UDPAddr)
	if !ok {
		return 0, errors.New("libcConn: not a UDP address")
	}
	ip := ipBytes(udp.IP)
	if len(c.local.IP) == net.IPv6len && len(ip) == net.IPv4len {
		ip = udp.IP.To16()
	}
	var buf unsafe.Pointer
	if len(b) > 0 {
		buf = unsafe.Pointer(&b[0])
	}
	n := C.nq_sendto(c.fd, buf, C.size_t(len(b)), (*C.uint8_t)(unsafe.Pointer(&ip[0])),
		C.int(len(ip)), C.uint16_t(udp.Port))
	if n < 0 {
		return 0, errnoErr("sendto", C.long(n))
	}
	return int(n), nil
}

func (c *libcConn) Close() error {
	if c.closed.Swap(true) {
		return nil
	}
	// Readers notice within pollInterval; close the fd only after that.
	time.AfterFunc(2*pollInterval, func() { C.close(c.fd) })
	return nil
}

func (c *libcConn) LocalAddr() net.Addr { return c.local }

func (c *libcConn) SetDeadline(t time.Time) error { return c.SetReadDeadline(t) }

func (c *libcConn) SetReadDeadline(t time.Time) error {
	c.mu.Lock()
	c.readDeadline = t
	c.mu.Unlock()
	return nil
}

// SetReadBuffer and SetWriteBuffer let quic-go size the socket buffers as it
// does for its own sockets.
func (c *libcConn) SetReadBuffer(bytes int) error {
	if rv := C.nq_setbuf(c.fd, C.SO_RCVBUF, C.int(bytes)); rv < 0 {
		return errnoErr("setsockopt", C.long(rv))
	}
	return nil
}

func (c *libcConn) SetWriteBuffer(bytes int) error {
	if rv := C.nq_setbuf(c.fd, C.SO_SNDBUF, C.int(bytes)); rv < 0 {
		return errnoErr("setsockopt", C.long(rv))
	}
	return nil
}

// Writes never block for long on UDP, so write deadlines are not enforced.
func (c *libcConn) SetWriteDeadline(time.Time) error { return nil }

// exit terminates through libc's exit() so atexit handlers run, which is
// where libnesquic.so reports its metrics. Go's own os.Exit and returning
// from main both end the process with a raw exit_group syscall instead.
func exit(code int) {
	C.exit(C.int(code))
}
