/* Shared xquic setup for the nesquic xquic IUT. */
#ifndef NESQUIC_XQUIC_COMMON_H
#define NESQUIC_XQUIC_COMMON_H

#include <xquic/xquic.h>
#include <xquic/xqc_errno.h>

#include "nesquic.h"

/* Idle timeout in milliseconds, matching the other C IUTs. */
#define NQ_IDLE_TIMEOUT_MS 10000

/*
 * xquic rejects a stream with more than 8192 buffered out-of-order frames, but
 * its receive window auto-tunes up to 16 MiB (~14k packets), so a single loss
 * early in a large transfer kills the connection. The client therefore pins
 * its window to this size: a stream "rate limit" of 1 B/s makes xquic use
 * max(init_recv_window, rate * srtt) = init_recv_window throughout.
 */
#define NQ_STREAM_RECV_WINDOW (6 * 1024 * 1024)

/* Set by SIGINT/SIGTERM; the event loops exit once it is set. */
extern volatile int nq_stop;

/* The UDP socket and timer shared by the engine callbacks and the event loop. */
struct nq_loop {
    int fd;
    struct sockaddr_storage local_addr;
    socklen_t local_addrlen;
    uint64_t timer_deadline; /* wall-clock microseconds, 0 if unarmed */
    int blocked;             /* the last send hit EAGAIN; wait for POLLOUT */
};

extern struct nq_loop nq_loop;

void nq_install_signal_handlers(void);

/* Wall-clock microseconds, the clock xquic uses internally by default. */
uint64_t nq_now_us(void);

/* Engine callbacks shared by client and server. */
void nq_set_event_timer(xqc_usec_t wake_after, void *engine_user_data);
void nq_log_write(xqc_log_level_t lvl, const void *buf, size_t size, void *engine_user_data);
ssize_t nq_write_socket(const unsigned char *buf, size_t size, const struct sockaddr *peer,
                        socklen_t peerlen, void *conn_user_data);

/* Connection settings shared by client and server. */
void nq_conn_settings(xqc_conn_settings_t *settings);

/*
 * Runs the engine until nq_stop is set or `done` (if non-NULL) becomes
 * non-zero. `on_writable` is called when the socket drains after EAGAIN.
 */
int nq_event_loop(xqc_engine_t *engine, const int *done,
                  void (*on_writable)(xqc_engine_t *engine));

int nq_run_client(const struct nq_args *args);
int nq_run_server(const struct nq_args *args);

#endif
