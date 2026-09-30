#include "common.h"

#include <errno.h>
#include <limits.h>
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

static void read_packets(xqc_engine_t *engine) {
    unsigned char buf[65536];
    unsigned batch = 0;

    for (;;) {
        struct sockaddr_storage from;
        socklen_t fromlen = sizeof(from);
        ssize_t n = recvfrom(nq_loop.fd, buf, sizeof(buf), MSG_DONTWAIT,
                             (struct sockaddr *)&from, &fromlen);
        if (n == -1) {
            if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR) {
                fprintf(stderr, "recvfrom: %s\n", strerror(errno));
            }
            break;
        }
        xqc_engine_packet_process(engine, buf, (size_t)n,
                                  (struct sockaddr *)&nq_loop.local_addr, nq_loop.local_addrlen,
                                  (struct sockaddr *)&from, fromlen, nq_now_us(), NULL);
        if (++batch == NQ_RECV_BATCH) {
            xqc_engine_finish_recv(engine);
            batch = 0;
        }
    }
    xqc_engine_finish_recv(engine);
}

int nq_event_loop(xqc_engine_t *engine, const int *done,
                  void (*on_writable)(xqc_engine_t *engine)) {
    while (!nq_stop && !(done && *done)) {
        struct pollfd pfd = {.fd = nq_loop.fd, .events = POLLIN};
        int timeout = -1, n;

        if (nq_loop.blocked) {
            pfd.events |= POLLOUT;
        }
        if (nq_loop.timer_deadline) {
            uint64_t now = nq_now_us();
            uint64_t ms = nq_loop.timer_deadline <= now
                              ? 0
                              : (nq_loop.timer_deadline - now + 999) / 1000;
            timeout = ms > INT_MAX ? INT_MAX : (int)ms;
        }

        n = poll(&pfd, 1, timeout);
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
