/*
 * Shared helpers for the C/C++ IUTs: the nesquic perf protocol
 * (docs/PROTOCOL.md) and the container CLI (docs/CLI.md).
 *
 * Header-only so every IUT can include it without an extra build step.
 */
#ifndef NESQUIC_H
#define NESQUIC_H

#include <arpa/inet.h>
#include <netdb.h>
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

enum nq_mode { NQ_CLIENT, NQ_SERVER };

/* Parsed command-line arguments shared by client and server. */
struct nq_args {
    enum nq_mode mode;
    const char *cert;   /* --cert: PEM certificate path */
    const char *key;    /* --key: PEM private key path (server only) */
    const char *blob;   /* --blob: requested size, e.g. "50Mbit" (client only) */
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
            "  %s client [-j JOB] [-L LABEL] --cert PEM --blob SIZE [URL]\n"
            "  %s server [-j JOB] [-L LABEL] --cert PEM --key PEM [LISTEN]\n",
            prog, prog);
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

        if (!strcmp(arg, "-c") || !strcmp(arg, "--cert")) {
            target = &args->cert;
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
        } else if (!strcmp(arg, "--unencrypted")) {
            /* Accepted but ignored, matching the other IUTs. */
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
        *target = argv[i];
    }

    if (args->mode == NQ_CLIENT) {
        if (!args->cert || !args->blob) {
            fprintf(stderr, "client requires --cert and --blob\n");
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

#ifdef __cplusplus
}
#endif

#endif /* NESQUIC_H */
