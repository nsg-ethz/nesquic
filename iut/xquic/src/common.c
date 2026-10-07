#define _GNU_SOURCE /* sendmmsg, ppoll */
#include "common.h"

#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <sys/time.h>

volatile int nq_stop = 0;
struct nq_loop nq_loop = {.fd = -1};

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

uint64_t nq_now_us(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (uint64_t)tv.tv_sec * 1000000 + (uint64_t)tv.tv_usec;
}

void nq_set_event_timer(xqc_usec_t wake_after, void *engine_user_data) {
    (void)engine_user_data;
    nq_loop.timer_deadline = nq_now_us() + wake_after;
}

void nq_log_write(xqc_log_level_t lvl, const void *buf, size_t size, void *engine_user_data) {
    (void)engine_user_data;
    /* Skip the per-connection XQC_LOG_REPORT summaries. */
    if (lvl == XQC_LOG_REPORT) {
        return;
    }
    fprintf(stderr, "%.*s\n", (int)size, (const char *)buf);
}

ssize_t nq_write_socket(const unsigned char *buf, size_t size, const struct sockaddr *peer,
                        socklen_t peerlen, void *conn_user_data) {
    ssize_t n;
    (void)conn_user_data;

    do {
        n = sendto(nq_loop.fd, buf, size, 0, peer, peerlen);
    } while (n == -1 && errno == EINTR);

    if (n == -1) {
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            nq_loop.blocked = 1;
            return XQC_SOCKET_EAGAIN;
        }
        fprintf(stderr, "sendto: %s\n", strerror(errno));
        return XQC_SOCKET_ERROR;
    }
    return n;
}

ssize_t nq_write_mmsg(const struct iovec *msg_iov, unsigned int vlen,
                      const struct sockaddr *peer, socklen_t peerlen, void *conn_user_data) {
    struct mmsghdr msgs[XQC_MAX_SEND_MSG_ONCE];
    int n;
    (void)conn_user_data;

    if (vlen > XQC_MAX_SEND_MSG_ONCE) {
        vlen = XQC_MAX_SEND_MSG_ONCE;
    }
    memset(msgs, 0, vlen * sizeof(msgs[0]));
    for (unsigned int i = 0; i < vlen; ++i) {
        msgs[i].msg_hdr.msg_name = (void *)peer;
        msgs[i].msg_hdr.msg_namelen = peerlen;
        msgs[i].msg_hdr.msg_iov = (struct iovec *)&msg_iov[i];
        msgs[i].msg_hdr.msg_iovlen = 1;
    }

    do {
        n = sendmmsg(nq_loop.fd, msgs, vlen, 0);
    } while (n == -1 && errno == EINTR);

    if (n == -1) {
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            nq_loop.blocked = 1;
            return XQC_SOCKET_EAGAIN;
        }
        fprintf(stderr, "sendmmsg: %s\n", strerror(errno));
        return XQC_SOCKET_ERROR;
    }
    if ((unsigned int)n < vlen) {
        nq_loop.blocked = 1;
    }
    return n;
}

void nq_conn_settings(xqc_conn_settings_t *settings) {
    memset(settings, 0, sizeof(*settings));
    settings->pacing_on = 1;
    settings->cong_ctrl_callback = xqc_cubic_cb;
    settings->proto_version = XQC_VERSION_V1;
    settings->idle_time_out = NQ_IDLE_TIMEOUT_MS;
    settings->init_idle_time_out = NQ_IDLE_TIMEOUT_MS;
    /* Lets the client pin its stream receive window (see NQ_STREAM_RECV_WINDOW). */
    settings->enable_stream_rate_limit = 1;
    settings->init_recv_window = NQ_STREAM_RECV_WINDOW;
}

/*
 * Datagrams handed to xquic between two xqc_engine_finish_recv() calls. Stream
 * data is only delivered to the application there, and xquic rejects a stream
 * with more than 8192 buffered frames, so a large backlog must be split.
 */
#define NQ_RECV_BATCH 32

struct recv_ctx {
    xqc_engine_t *engine;
    unsigned batch;
};

static void packet_in(void *ctx, uint8_t *data, size_t len, struct sockaddr *from,
                      socklen_t fromlen) {
    struct recv_ctx *r = ctx;

    xqc_engine_packet_process(r->engine, data, len, (struct sockaddr *)&nq_loop.local_addr,
                              nq_loop.local_addrlen, from, fromlen, nq_now_us(), NULL);
    if (++r->batch == NQ_RECV_BATCH) {
        xqc_engine_finish_recv(r->engine);
        r->batch = 0;
    }
}

static void read_packets(xqc_engine_t *engine) {
    struct recv_ctx r = {.engine = engine};

    nq_recv_packets(nq_loop.fd, packet_in, &r);
    xqc_engine_finish_recv(engine);
}

int nq_event_loop(xqc_engine_t *engine, const int *done,
                  void (*on_writable)(xqc_engine_t *engine)) {
    nq_socket_setup(nq_loop.fd);

    while (!nq_stop && !(done && *done)) {
        struct pollfd pfd = {.fd = nq_loop.fd, .events = POLLIN};
        struct timespec ts = {0}, *timeout = NULL;
        int n;

        if (nq_loop.blocked) {
            pfd.events |= POLLOUT;
        }
        if (nq_loop.timer_deadline) {
            /* Microsecond resolution: pacing needs sub-millisecond timers. */
            uint64_t now = nq_now_us();
            uint64_t us = nq_loop.timer_deadline <= now ? 0 : nq_loop.timer_deadline - now;
            ts.tv_sec = (time_t)(us / 1000000);
            ts.tv_nsec = (long)(us % 1000000) * 1000;
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
            nq_loop.blocked = 0;
            on_writable(engine);
        }
        if (n > 0 && (pfd.revents & POLLIN)) {
            read_packets(engine);
        }
        if (nq_loop.timer_deadline && nq_loop.timer_deadline <= nq_now_us()) {
            nq_loop.timer_deadline = 0;
            xqc_engine_main_logic(engine);
        }
    }
    return 0;
}
