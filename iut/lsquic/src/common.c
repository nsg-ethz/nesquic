#define _GNU_SOURCE /* sendmmsg, ppoll */
#include "common.h"

#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <sys/uio.h>

/* Datagrams handed to the kernel with one sendmmsg. */
#define NQ_SEND_BATCH 64

volatile int nq_stop = 0;

static void on_signal(int sig) {
    (void)sig;
    nq_stop = 1;
}

void nq_install_signal_handlers(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = on_signal;
    /* No SA_RESTART: poll() must return EINTR so the loop notices nq_stop. */
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGTERM, &sa, NULL);
}

void nq_engine_settings(struct lsquic_engine_settings *settings, unsigned flags) {
    lsquic_engine_init_settings(settings, flags);
    settings->es_versions = 1 << LSQVER_I001;
    settings->es_idle_timeout = NQ_IDLE_TIMEOUT_S;
    settings->es_cc_algo = 1; /* Cubic; the default picks Cubic or BBR by RTT */
    /* Fixed windows: the maxima bound auto-tuning. */
    settings->es_cfcw = settings->es_max_cfcw = NQ_CONNECTION_WINDOW;
    settings->es_sfcw = settings->es_max_sfcw = NQ_STREAM_WINDOW;
    settings->es_init_max_data = NQ_CONNECTION_WINDOW;
    settings->es_init_max_stream_data_bidi_local = NQ_STREAM_WINDOW;
    settings->es_init_max_stream_data_bidi_remote = NQ_STREAM_WINDOW;
    settings->es_init_max_streams_bidi = NQ_MAX_STREAMS;
}

static socklen_t sockaddr_len(const struct sockaddr *sa) {
    return sa->sa_family == AF_INET6 ? sizeof(struct sockaddr_in6)
                                     : sizeof(struct sockaddr_in);
}

int nq_packets_out(void *ctx, const struct lsquic_out_spec *specs, unsigned n_specs) {
    struct nq_socket *sock = ctx;
    struct mmsghdr msgs[NQ_SEND_BATCH];
    unsigned sent = 0;

    while (sent < n_specs) {
        unsigned n = n_specs - sent < NQ_SEND_BATCH ? n_specs - sent : NQ_SEND_BATCH;
        int rv;

        memset(msgs, 0, n * sizeof(msgs[0]));
        for (unsigned i = 0; i < n; ++i) {
            const struct lsquic_out_spec *spec = &specs[sent + i];
            msgs[i].msg_hdr.msg_name = (void *)spec->dest_sa;
            msgs[i].msg_hdr.msg_namelen = sockaddr_len(spec->dest_sa);
            msgs[i].msg_hdr.msg_iov = spec->iov;
            msgs[i].msg_hdr.msg_iovlen = spec->iovlen;
        }

        do {
            rv = sendmmsg(sock->fd, msgs, n, 0);
        } while (rv == -1 && errno == EINTR);

        if (rv > 0) {
            sent += (unsigned)rv;
        }
        if (rv < (int)n) {
            /* lsquic inspects errno and retries once we report writability. */
            if (rv >= 0) {
                errno = EAGAIN;
            }
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                sock->blocked = 1;
            }
            break;
        }
    }
    return sent > 0 ? (int)sent : -1;
}

static void packet_in(void *ctx, uint8_t *data, size_t len, struct sockaddr *from,
                      socklen_t fromlen) {
    struct nq_socket *sock = ctx;
    (void)fromlen;
    lsquic_engine_packet_in(sock->engine, data, len, (struct sockaddr *)&sock->local_addr, from,
                            sock, 0);
}

int nq_event_loop(lsquic_engine_t *engine, struct nq_socket *sock, const int *done) {
    sock->engine = engine;
    nq_socket_setup(sock->fd);
    lsquic_engine_process_conns(engine);

    while (!nq_stop && !(done && *done)) {
        struct pollfd pfd = {.fd = sock->fd, .events = POLLIN};
        struct timespec ts = {0}, *timeout = NULL;
        int diff, n;

        if (sock->blocked) {
            pfd.events |= POLLOUT;
        }
        if (lsquic_engine_earliest_adv_tick(engine, &diff)) {
            /* diff is in microseconds: pacing needs sub-millisecond timers. */
            if (diff > 0) {
                ts.tv_sec = diff / 1000000;
                ts.tv_nsec = (long)(diff % 1000000) * 1000;
            }
            timeout = &ts;
        }

        n = ppoll(&pfd, 1, timeout, NULL);
        if (nq_stop) {
            break;
        }
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            fprintf(stderr, "poll: %s\n", strerror(errno));
            return -1;
        }

        if (n > 0 && (pfd.revents & POLLOUT)) {
            sock->blocked = 0;
            lsquic_engine_send_unsent_packets(engine);
        }
        if (n > 0 && (pfd.revents & POLLIN) && nq_recv_packets(sock->fd, packet_in, sock) != 0) {
            return -1;
        }
        lsquic_engine_process_conns(engine);
    }
    return 0;
}
