/* Shared lsquic setup for the nesquic lsquic IUT. */
#ifndef NESQUIC_LSQUIC_COMMON_H
#define NESQUIC_LSQUIC_COMMON_H

#include <lsquic.h>

#include "nesquic.h"

/* Idle timeout in seconds, matching the other C IUTs. */
#define NQ_IDLE_TIMEOUT_S 10

/* Set by SIGINT/SIGTERM; the event loops exit once it is set. */
extern volatile int nq_stop;

/* Installs SIGINT/SIGTERM handlers that set nq_stop and interrupt poll(). */
void nq_install_signal_handlers(void);

/* The UDP socket shared by the engine callbacks and the event loop. */
struct nq_socket {
    int fd;
    lsquic_engine_t *engine;
    struct sockaddr_storage local_addr;
    socklen_t local_addrlen;
    int blocked; /* the last send hit EAGAIN; wait for POLLOUT */
};

/* lsquic ea_packets_out callback; ctx is a struct nq_socket. */
int nq_packets_out(void *ctx, const struct lsquic_out_spec *specs, unsigned n_specs);

/* Settings shared by client and server (see docs/PROTOCOL.md). */
void nq_engine_settings(struct lsquic_engine_settings *settings, unsigned flags);

/*
 * Runs the engine until nq_stop is set or `done` (if non-NULL) becomes
 * non-zero. Returns 0 on success.
 */
int nq_event_loop(lsquic_engine_t *engine, struct nq_socket *sock, const int *done);

/* Runs one connection (see nq_run_connections). */
int nq_run_client(const struct nq_args *args, struct nq_load *load);
int nq_run_server(const struct nq_args *args);

#endif
