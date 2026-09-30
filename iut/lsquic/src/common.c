#include "common.h"

#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <sys/uio.h>

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

static socklen_t sockaddr_len(const struct sockaddr *sa) {
    return sa->sa_family == AF_INET6 ? sizeof(struct sockaddr_in6)
                                     : sizeof(struct sockaddr_in);
}

int nq_packets_out(void *ctx, const struct lsquic_out_spec *specs, unsigned n_specs) {
    struct nq_socket *sock = ctx;
    unsigned i;

    for (i = 0; i < n_specs; ++i) {
        struct msghdr msg;
        ssize_t n;

        memset(&msg, 0, sizeof(msg));
        msg.msg_name = (void *)specs[i].dest_sa;
        msg.msg_namelen = sockaddr_len(specs[i].dest_sa);
        msg.msg_iov = specs[i].iov;
        msg.msg_iovlen = specs[i].iovlen;

        do {
            n = sendmsg(sock->fd, &msg, 0);
        } while (n == -1 && errno == EINTR);

        if (n == -1) {
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                sock->blocked = 1;
            }
            /* lsquic inspects errno and retries once we report writability. */
            return i > 0 ? (int)i : -1;
        }
    }
    return (int)n_specs;
}

int nq_read_packets(lsquic_engine_t *engine, struct nq_socket *sock) {
    unsigned char buf[65536];

    for (;;) {
        struct sockaddr_storage from;
        socklen_t fromlen = sizeof(from);
        ssize_t n = recvfrom(sock->fd, buf, sizeof(buf), MSG_DONTWAIT,
                             (struct sockaddr *)&from, &fromlen);
        if (n == -1) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                return 0;
            }
            fprintf(stderr, "recvfrom: %s\n", strerror(errno));
            return -1;
        }
        lsquic_engine_packet_in(engine, buf, (size_t)n,
                                (struct sockaddr *)&sock->local_addr,
                                (struct sockaddr *)&from, sock, 0);
    }
}

int nq_event_loop(lsquic_engine_t *engine, struct nq_socket *sock, const int *done) {
    lsquic_engine_process_conns(engine);

    while (!nq_stop && !(done && *done)) {
        struct pollfd pfd = {.fd = sock->fd, .events = POLLIN};
        int timeout = -1, diff, n;

        if (sock->blocked) {
            pfd.events |= POLLOUT;
        }
        if (lsquic_engine_earliest_adv_tick(engine, &diff)) {
            /* diff is in microseconds; round up to whole milliseconds. */
            timeout = diff <= 0 ? 0 : (diff + 999) / 1000;
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
            sock->blocked = 0;
            lsquic_engine_send_unsent_packets(engine);
        }
        if (n > 0 && (pfd.revents & POLLIN) && nq_read_packets(engine, sock) != 0) {
            return -1;
        }
        lsquic_engine_process_conns(engine);
    }
    return 0;
}
