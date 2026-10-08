/*
 * Shared helpers for the C/C++ IUTs: the nesquic perf protocol
 * (docs/PROTOCOL.md) and the container CLI (docs/CLI.md).
 *
 * Header-only so every IUT can include it without an extra build step.
 */
#ifndef NESQUIC_H
#define NESQUIC_H

#include <arpa/inet.h>
#include <errno.h>
#include <netdb.h>
#include <limits.h>
#include <netinet/udp.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ALPN required by the perf protocol, as a string and in wire format. */
#define NQ_ALPN "perf"
#define NQ_ALPN_WIRE "\x04perf"
#define NQ_DEFAULT_PORT 4433
#define NQ_DEFAULT_URL "https://127.0.0.1:4433"
#define NQ_DEFAULT_LISTEN "0.0.0.0:4433"
#define NQ_REQUEST_LEN 8

/* Transport settings every IUT applies (see docs/PROTOCOL.md). */
#define NQ_STREAM_WINDOW (8 * 1024 * 1024)
#define NQ_CONNECTION_WINDOW (16 * 1024 * 1024)
#define NQ_MAX_STREAMS 100
/* Requested SO_RCVBUF/SO_SNDBUF; the kernel clamps it to net.core.{r,w}mem_max. */
#define NQ_SOCKET_BUFFER (16 * 1024 * 1024)
/* The response is served in chunks of a zero buffer of this size. */
#define NQ_ZERO_CHUNK (64 * 1024)

enum nq_mode { NQ_CLIENT, NQ_SERVER };

/* Parsed command-line arguments shared by client and server. */
struct nq_args {
    enum nq_mode mode;
    const char *cert;   /* --cert: PEM certificate path */
    const char *key;    /* --key: PEM private key path (server only) */
    const char *blob;   /* --blob: requested size, e.g. "50Mbit" (client only) */
    unsigned connections; /* --connections: QUIC connections (client only) */
    unsigned streams;     /* --streams: concurrent requests per connection (client only) */
    unsigned duration;    /* --duration: seconds of requests, 0 for one round (client only) */
    const char *url;    /* client positional: server URL */
    const char *listen; /* server positional: listen address:port */
};

/*
 * Parses a blob-size string "<number>[G|M|K]bit" into a byte count
 * (bits / 8, see docs/CLI.md). Returns 0 on success.
 */
static inline int nq_blob_bytes(const char *value, uint64_t *out) {
    size_t len = strlen(value);
    uint64_t mult = 1, n = 0;
    size_t num_end, i;
    char prefix;

    if (len < 4 || strcmp(value + len - 3, "bit") != 0) {
        return -1;
    }

    prefix = value[len - 4];
    if (prefix >= '0' && prefix <= '9') {
        num_end = len - 3;
    } else {
        switch (prefix) {
            case 'G': mult = 1000ULL * 1000 * 1000; break;
            case 'M': mult = 1000ULL * 1000; break;
            case 'K': mult = 1000ULL; break;
            default: return -1;
        }
        num_end = len - 4;
    }

    if (num_end == 0) {
        return -1;
    }

    for (i = 0; i < num_end; ++i) {
        if (value[i] < '0' || value[i] > '9') {
            return -1;
        }
        n = n * 10 + (uint64_t)(value[i] - '0');
    }

    *out = n * mult / 8;
    return 0;
}

/* Serializes a byte count as the fixed 8-byte big-endian request. */
static inline void nq_request_encode(uint64_t size, uint8_t out[NQ_REQUEST_LEN]) {
    int i;
    for (i = 0; i < NQ_REQUEST_LEN; ++i) {
        out[NQ_REQUEST_LEN - 1 - i] = (uint8_t)(size >> (8 * i));
    }
}

/* Parses the 8-byte big-endian request into a byte count. */
static inline uint64_t nq_request_decode(const uint8_t in[NQ_REQUEST_LEN]) {
    uint64_t size = 0;
    int i;
    for (i = 0; i < NQ_REQUEST_LEN; ++i) {
        size = (size << 8) | in[i];
    }
    return size;
}

static inline void nq_usage(const char *prog) {
    fprintf(stderr,
            "usage:\n"
            "  %s client [-j JOB] [-L LABEL] --cert PEM --blob SIZE [-c CONNECTIONS]\n"
            "      [-s STREAMS] [-d SECONDS] [URL]\n"
            "  %s server [-j JOB] [-L LABEL] --cert PEM --key PEM [LISTEN]\n",
            prog, prog);
}

/* Parses a positive count. Returns 0 on success. */
static inline int nq_parse_count(const char *value, unsigned *out) {
    char *end;
    unsigned long n;

    errno = 0;
    n = strtoul(value, &end, 10);
    if (value[0] < '0' || value[0] > '9' || *end || errno || n == 0 || n > UINT_MAX) {
        return -1;
    }
    *out = (unsigned)n;
    return 0;
}

/*
 * Parses the container CLI. Returns 0 on success, or the exit code to use on
 * failure. `-j`/`-L` are read by libnesquic.so from /proc/self/cmdline, so
 * they are accepted and ignored here.
 */
static inline int nq_parse_args(int argc, char **argv, struct nq_args *args) {
    const char *positional = NULL;
    int i;

    memset(args, 0, sizeof(*args));
    args->connections = args->streams = 1;
    if (argc < 2) {
        nq_usage(argv[0]);
        return 2;
    }
    if (strcmp(argv[1], "client") == 0) {
        args->mode = NQ_CLIENT;
    } else if (strcmp(argv[1], "server") == 0) {
        args->mode = NQ_SERVER;
    } else {
        nq_usage(argv[0]);
        return 2;
    }

    for (i = 2; i < argc; ++i) {
        const char *arg = argv[i];
        const char **target = NULL;
        unsigned *count = NULL;

        /* -c is the certificate for servers and the connections for clients. */
        if ((!strcmp(arg, "-c") && args->mode == NQ_SERVER) || !strcmp(arg, "--cert")) {
            target = &args->cert;
        } else if (!strcmp(arg, "-c") || !strcmp(arg, "--connections")) {
            count = &args->connections;
        } else if (!strcmp(arg, "-s") || !strcmp(arg, "--streams")) {
            count = &args->streams;
        } else if (!strcmp(arg, "-d") || !strcmp(arg, "--duration")) {
            count = &args->duration;
        } else if (!strcmp(arg, "-k") || !strcmp(arg, "--key")) {
            target = &args->key;
        } else if (!strcmp(arg, "-b") || !strcmp(arg, "--blob")) {
            target = &args->blob;
        } else if (!strcmp(arg, "-j") || !strcmp(arg, "--job") || !strcmp(arg, "-L") ||
                   !strcmp(arg, "--labels") || !strcmp(arg, "-l") || !strcmp(arg, "--lib") ||
                   !strcmp(arg, "--quic-cpu")) {
            if (++i >= argc) {
                nq_usage(argv[0]);
                return 2;
            }
            continue;
        } else if (!strncmp(arg, "-j", 2) || !strncmp(arg, "-L", 2) ||
                   !strncmp(arg, "--job=", 6) || !strncmp(arg, "--labels=", 9)) {
            continue;
        } else if (arg[0] == '-') {
            fprintf(stderr, "unknown option: %s\n", arg);
            nq_usage(argv[0]);
            return 2;
        } else {
            positional = arg;
            continue;
        }

        if (++i >= argc) {
            nq_usage(argv[0]);
            return 2;
        }
        if (!count) {
            *target = argv[i];
        } else if (nq_parse_count(argv[i], count) != 0) {
            fprintf(stderr, "%s requires a positive number\n", arg);
            return 2;
        }
    }

    if (args->mode == NQ_CLIENT) {
        if (!args->cert || !args->blob) {
            fprintf(stderr, "client requires --cert and --blob\n");
            return 2;
        }
        if (args->streams > NQ_MAX_STREAMS) {
            fprintf(stderr, "--streams is at most %d\n", NQ_MAX_STREAMS);
            return 2;
        }
        args->url = positional ? positional : NQ_DEFAULT_URL;
    } else {
        if (!args->cert || !args->key) {
            fprintf(stderr, "server requires --cert and --key\n");
            return 2;
        }
        args->listen = positional ? positional : NQ_DEFAULT_LISTEN;
    }
    return 0;
}

/*
 * Splits "https://host:port/path" (client URL) or "host:port" (listen
 * address) into host and port. IPv6 literals may be bracketed. The port
 * defaults to 4433. Returns 0 on success.
 */
static inline int nq_split_host_port(const char *in, char *host, size_t hostlen,
                                     uint16_t *port) {
    const char *rest = strstr(in, "://");
    const char *end, *colon = NULL;
    size_t n;

    rest = rest ? rest + 3 : in;
    end = rest + strcspn(rest, "/");

    if (*rest == '[') {
        const char *close = (const char *)memchr(rest, ']', (size_t)(end - rest));
        if (!close) {
            return -1;
        }
        n = (size_t)(close - rest - 1);
        if (n >= hostlen) {
            return -1;
        }
        memcpy(host, rest + 1, n);
        host[n] = '\0';
        if (close + 1 < end && close[1] == ':') {
            colon = close + 1;
        }
    } else {
        const char *p;
        for (p = rest; p < end; ++p) {
            if (*p == ':') {
                colon = p;
            }
        }
        n = (size_t)((colon ? colon : end) - rest);
        if (n >= hostlen) {
            return -1;
        }
        memcpy(host, rest, n);
        host[n] = '\0';
    }

    *port = NQ_DEFAULT_PORT;
    if (colon) {
        long p = strtol(colon + 1, NULL, 10);
        if (p <= 0 || p > 65535) {
            return -1;
        }
        *port = (uint16_t)p;
    }
    return 0;
}

/* Resolves host:port to a UDP socket address. Returns 0 on success. */
static inline int nq_resolve(const char *host, uint16_t port, struct sockaddr_storage *addr,
                             socklen_t *addrlen) {
    struct addrinfo hints, *res;
    char service[8];
    int rv;

    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_DGRAM;
    hints.ai_flags = AI_PASSIVE;
    snprintf(service, sizeof(service), "%u", port);

    rv = getaddrinfo(host[0] ? host : NULL, service, &hints, &res);
    if (rv != 0) {
        fprintf(stderr, "getaddrinfo(%s): %s\n", host, gai_strerror(rv));
        return -1;
    }
    memcpy(addr, res->ai_addr, res->ai_addrlen);
    *addrlen = (socklen_t)res->ai_addrlen;
    freeaddrinfo(res);
    return 0;
}

/* Applies the shared UDP socket setup: buffer sizes and UDP_GRO (best effort). */
static inline void nq_socket_setup(int fd) {
    int size = NQ_SOCKET_BUFFER, on = 1;
    setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &size, sizeof(size));
    setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &size, sizeof(size));
    setsockopt(fd, SOL_UDP, UDP_GRO, &on, sizeof(on));
}

typedef void (*nq_recv_cb)(void *ctx, uint8_t *data, size_t len, struct sockaddr *from,
                           socklen_t fromlen);

/*
 * Drains the socket, calling `cb` for every datagram. A read may return
 * several datagrams coalesced by UDP_GRO (see nq_socket_setup), which are
 * split here. Returns 0 on success.
 */
static inline int nq_recv_packets(int fd, nq_recv_cb cb, void *ctx) {
    /* Per thread: every client connection reads on its own (nq_run_connections). */
    static __thread uint8_t buf[65536];

    for (;;) {
        struct sockaddr_storage from;
        char control[CMSG_SPACE(sizeof(int))];
        struct iovec iov;
        struct msghdr msg;
        struct cmsghdr *cmsg;
        ssize_t n, off;
        size_t segment;

        iov.iov_base = buf;
        iov.iov_len = sizeof(buf);
        memset(&msg, 0, sizeof(msg));
        msg.msg_name = &from;
        msg.msg_namelen = sizeof(from);
        msg.msg_iov = &iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control;
        msg.msg_controllen = sizeof(control);

        n = recvmsg(fd, &msg, MSG_DONTWAIT);
        if (n == -1) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                return 0;
            }
            fprintf(stderr, "recvmsg: %s\n", strerror(errno));
            return -1;
        }
        if (n == 0) {
            continue;
        }

        segment = (size_t)n;
        for (cmsg = CMSG_FIRSTHDR(&msg); cmsg; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
            if (cmsg->cmsg_level == SOL_UDP && cmsg->cmsg_type == UDP_GRO) {
                int gro_size;
                memcpy(&gro_size, CMSG_DATA(cmsg), sizeof(gro_size));
                if (gro_size > 0) {
                    segment = (size_t)gro_size;
                }
            }
        }
        for (off = 0; off < n; off += (ssize_t)segment) {
            size_t len = (size_t)(n - off) < segment ? (size_t)(n - off) : segment;
            cb(ctx, buf + off, len, (struct sockaddr *)&from, msg.msg_namelen);
        }
    }
}

/* Whether `host` is an IPv4 or IPv6 literal (no SNI is sent for those). */
static inline int nq_is_ip_literal(const char *host) {
    uint8_t buf[sizeof(struct in6_addr)];
    return inet_pton(AF_INET, host, buf) == 1 || inet_pton(AF_INET6, host, buf) == 1;
}

/* Monotonic clock in nanoseconds. */
static inline uint64_t nq_now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

/*
 * The requests of one client connection (see docs/CLI.md): `streams` run
 * concurrently and, with --duration, each is followed by another until the
 * deadline. Used from the connection's thread only.
 */
struct nq_load {
    uint8_t request[NQ_REQUEST_LEN];
    uint64_t requested;   /* bytes expected in every response */
    uint64_t duration_ns;
    unsigned pending;     /* requests the client has yet to start */
    unsigned slots;       /* concurrent requests that have not finished */
    int failed;
    uint64_t first_ns, last_ns, deadline_ns;
    uint64_t requests, bytes, latency_ns;
};

/* Returns 0 on success. */
static inline int nq_load_init(struct nq_load *l, const char *blob, unsigned streams,
                               unsigned duration) {
    memset(l, 0, sizeof(*l));
    if (nq_blob_bytes(blob, &l->requested) != 0) {
        fprintf(stderr, "malformed blob size: %s\n", blob);
        return -1;
    }
    nq_request_encode(l->requested, l->request);
    l->pending = l->slots = streams;
    l->duration_ns = (uint64_t)duration * 1000000000ULL;
    return 0;
}

/* Call when a request is written. Returns its start time for nq_load_end. */
static inline uint64_t nq_load_begin(struct nq_load *l) {
    uint64_t now = nq_now_ns();
    if (!l->first_ns) {
        l->first_ns = now;
        l->deadline_ns = now + l->duration_ns;
    }
    return now;
}

/*
 * Call at the end of a response. Raises `pending` if another request follows;
 * otherwise the request's slot is finished.
 */
static inline void nq_load_end(struct nq_load *l, uint64_t received, uint64_t begin_ns) {
    uint64_t now = nq_now_ns();

    ++l->requests;
    l->bytes += received;
    l->latency_ns += now - begin_ns;
    l->last_ns = now;
    if (received != l->requested) {
        fprintf(stderr, "received blob size (%lluB) different from requested (%lluB)\n",
                (unsigned long long)received, (unsigned long long)l->requested);
        l->failed = 1;
    }
    if (!l->failed && now < l->deadline_ns) {
        ++l->pending;
    } else {
        --l->slots;
    }
}

/* Whether the connection has nothing left to do and should be closed. */
static inline int nq_load_done(const struct nq_load *l) {
    return l->failed || !l->slots;
}

static inline int nq_load_ok(const struct nq_load *l) {
    return !l->failed && !l->slots;
}

static inline void nq_load_merge(struct nq_load *total, const struct nq_load *l) {
    if (!l->requests) {
        return;
    }
    if (!total->requests || l->first_ns < total->first_ns) {
        total->first_ns = l->first_ns;
    }
    if (l->last_ns > total->last_ns) {
        total->last_ns = l->last_ns;
    }
    total->requests += l->requests;
    total->bytes += l->bytes;
    total->latency_ns += l->latency_ns;
}

/* Prints the client's own measurement of all its requests (docs/METRICS.md). */
static inline void nq_load_report(const struct nq_load *l) {
    double secs = (double)(l->last_ns - l->first_ns) / 1e9;
    if (!l->requests) {
        return;
    }
    printf("nesquic_app throughput=%f,request_latency_ms=%f,requests=%llu\n",
           (double)l->bytes / 1e6 / secs, (double)l->latency_ns / 1e6 / (double)l->requests,
           (unsigned long long)l->requests);
}

/* Runs one connection's requests. Returns the exit code. */
typedef int (*nq_connection_fn)(const struct nq_args *args, struct nq_load *load);

struct nq_connection {
    pthread_t thread;
    int started;
    const struct nq_args *args;
    nq_connection_fn run;
    struct nq_load load;
    int rc;
};

static inline void *nq_connection_main(void *arg) {
    struct nq_connection *c = (struct nq_connection *)arg;
    c->rc = c->run(c->args, &c->load);
    return NULL;
}

/*
 * Runs `run` once per --connections, each on its own thread and so with its
 * own socket and event loop, then reports their requests. For libraries whose
 * event loop drives a single socket. Returns the exit code.
 */
static inline int nq_run_connections(const struct nq_args *args, nq_connection_fn run) {
    struct nq_connection *conns =
        (struct nq_connection *)calloc(args->connections, sizeof(*conns));
    struct nq_load total;
    unsigned i;
    int rc = 0;

    memset(&total, 0, sizeof(total));
    if (!conns) {
        return 1;
    }
    for (i = 0; i < args->connections; ++i) {
        struct nq_connection *c = &conns[i];
        c->args = args;
        c->run = run;
        c->rc = 1;
        if (nq_load_init(&c->load, args->blob, args->streams, args->duration) != 0) {
            break;
        }
        c->started = pthread_create(&c->thread, NULL, nq_connection_main, c) == 0;
    }
    for (i = 0; i < args->connections; ++i) {
        if (conns[i].started) {
            pthread_join(conns[i].thread, NULL);
        }
        rc |= conns[i].rc;
        nq_load_merge(&total, &conns[i].load);
    }
    nq_load_report(&total);
    free(conns);
    return rc ? 1 : 0;
}

#ifdef __cplusplus
}
#endif

#endif /* NESQUIC_H */
