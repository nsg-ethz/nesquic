/*
 * Standalone picoquic client/server for the nesquic perf benchmark.
 * Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
 */
#include <signal.h>
#include <string.h>

#include <picoquic.h>
#include <picoquic_packet_loop.h>
#include <picoquic_utils.h>

#include "nesquic.h"

/* Longest the packet loop sleeps before checking for SIGINT/SIGTERM. */
#define NQ_SIGNAL_CHECK_US 100000
#define NQ_IDLE_TIMEOUT_MS 10000

static volatile sig_atomic_t nq_stop = 0;

static void on_signal(int sig) {
    (void)sig;
    nq_stop = 1;
}

/* ---- client ---- */

struct client {
    uint8_t request[NQ_REQUEST_LEN];
    uint64_t requested;
    uint64_t received;
    int ok;
    int disconnected;
};

static int client_callback(picoquic_cnx_t *cnx, uint64_t stream_id, uint8_t *bytes,
                           size_t length, picoquic_call_back_event_t event, void *ctx,
                           void *stream_ctx) {
    struct client *c = ctx;
    (void)stream_id;
    (void)bytes;
    (void)stream_ctx;

    switch (event) {
        case picoquic_callback_stream_data:
        case picoquic_callback_stream_fin:
            c->received += length;
            if (event == picoquic_callback_stream_fin) {
                c->ok = c->received == c->requested;
                if (!c->ok) {
                    fprintf(stderr,
                            "received blob size (%lluB) different from requested (%lluB)\n",
                            (unsigned long long)c->received, (unsigned long long)c->requested);
                }
                /* Single exchange done: close the connection (application close). */
                picoquic_close(cnx, 0);
            }
            break;
        case picoquic_callback_stream_reset:
            fprintf(stderr, "stream reset by server\n");
            picoquic_close(cnx, 0);
            break;
        case picoquic_callback_stateless_reset:
        case picoquic_callback_close:
        case picoquic_callback_application_close:
            if (!c->ok) {
                fprintf(stderr, "connection closed (local error 0x%llx, remote error 0x%llx)\n",
                        (unsigned long long)picoquic_get_local_error(cnx),
                        (unsigned long long)picoquic_get_remote_error(cnx));
            }
            c->disconnected = 1;
            picoquic_set_callback(cnx, NULL, NULL);
            break;
        default:
            break;
    }
    return 0;
}

static int client_loop_cb(picoquic_quic_t *quic, picoquic_packet_loop_cb_enum mode, void *ctx,
                          void *arg) {
    struct client *c = ctx;
    (void)quic;
    (void)arg;
    if (nq_stop) {
        return PICOQUIC_NO_ERROR_TERMINATE_PACKET_LOOP;
    }
    if (mode == picoquic_packet_loop_after_send && c->disconnected) {
        return PICOQUIC_NO_ERROR_TERMINATE_PACKET_LOOP;
    }
    return 0;
}

static int run_client(const struct nq_args *args) {
    struct client c;
    struct sockaddr_storage server;
    socklen_t serverlen;
    char host[256];
    uint16_t port;
    picoquic_quic_t *quic;
    picoquic_cnx_t *cnx;
    uint64_t now = picoquic_current_time();

    memset(&c, 0, sizeof(c));
    if (nq_blob_bytes(args->blob, &c.requested) != 0) {
        fprintf(stderr, "malformed blob size: %s\n", args->blob);
        return 1;
    }
    nq_request_encode(c.requested, c.request);
    if (nq_split_host_port(args->url, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed url: %s\n", args->url);
        return 1;
    }
    if (nq_resolve(host, port, &server, &serverlen) != 0) {
        return 1;
    }

    /* Trust only the supplied certificate (see docs/PROTOCOL.md). picotls
     * checks it against the SNI, which may be an IP literal. */
    quic = picoquic_create(1, NULL, NULL, args->cert, NQ_ALPN, NULL, NULL, NULL, NULL, NULL, now,
                           NULL, NULL, NULL, 0);
    if (!quic) {
        fprintf(stderr, "cannot create QUIC context\n");
        return 1;
    }
    picoquic_set_default_idle_timeout(quic, NQ_IDLE_TIMEOUT_MS);

    cnx = picoquic_create_cnx(quic, picoquic_null_connection_id, picoquic_null_connection_id,
                              (struct sockaddr *)&server, now, 0, host, NQ_ALPN, 1);
    if (!cnx) {
        fprintf(stderr, "cannot create connection\n");
        picoquic_free(quic);
        return 1;
    }
    picoquic_set_callback(cnx, client_callback, &c);

    /* Queue the request on the first client bidirectional stream, with FIN;
     * it is sent once the handshake allows. */
    if (picoquic_add_to_stream(cnx, 0, c.request, sizeof(c.request), 1) != 0 ||
        picoquic_start_client_cnx(cnx) != 0) {
        fprintf(stderr, "cannot start connection\n");
        picoquic_free(quic);
        return 1;
    }

    picoquic_packet_loop(quic, 0, server.ss_family, 0, 0, 0, client_loop_cb, &c);
    picoquic_free(quic);
    return c.ok && !nq_stop ? 0 : 1;
}

/* ---- server ---- */

struct stream {
    uint8_t request[NQ_REQUEST_LEN];
    size_t request_len;
    int responding;
    uint64_t remaining;
};

static void stream_free(picoquic_cnx_t *cnx, uint64_t stream_id, struct stream *st) {
    picoquic_unlink_app_stream_ctx(cnx, stream_id);
    free(st);
}

static int server_callback(picoquic_cnx_t *cnx, uint64_t stream_id, uint8_t *bytes, size_t length,
                           picoquic_call_back_event_t event, void *ctx, void *stream_ctx) {
    struct stream *st = stream_ctx;
    (void)ctx;

    switch (event) {
        case picoquic_callback_stream_data:
        case picoquic_callback_stream_fin:
            if (!st) {
                st = calloc(1, sizeof(*st));
                if (!st || picoquic_set_app_stream_ctx(cnx, stream_id, st) != 0) {
                    free(st);
                    picoquic_reset_stream(cnx, stream_id, 1);
                    return 0;
                }
            }
            /* Only the leading 8 bytes are significant (docs/PROTOCOL.md). */
            for (size_t i = 0; i < length && st->request_len < NQ_REQUEST_LEN; ++i) {
                st->request[st->request_len++] = bytes[i];
            }
            if (event == picoquic_callback_stream_fin && !st->responding) {
                if (st->request_len < NQ_REQUEST_LEN) {
                    stream_free(cnx, stream_id, st);
                    picoquic_reset_stream(cnx, stream_id, 1);
                    return 0;
                }
                st->responding = 1;
                st->remaining = nq_request_decode(st->request);
                if (st->remaining == 0) {
                    stream_free(cnx, stream_id, st);
                    picoquic_add_to_stream(cnx, stream_id, NULL, 0, 1);
                } else {
                    picoquic_mark_active_stream(cnx, stream_id, 1, st);
                }
            }
            break;
        case picoquic_callback_prepare_to_send:
            if (st && st->responding) {
                size_t n = length;
                int fin = 0;
                if (st->remaining <= n) {
                    n = (size_t)st->remaining;
                    fin = 1;
                }
                /* Response bytes are zeros written straight into the frame. */
                uint8_t *buf = picoquic_provide_stream_data_buffer(bytes, n, fin, !fin);
                if (buf) {
                    memset(buf, 0, n);
                    st->remaining -= n;
                    if (fin) {
                        stream_free(cnx, stream_id, st);
                    }
                }
            }
            break;
        case picoquic_callback_stream_reset:
        case picoquic_callback_stop_sending:
            if (st) {
                stream_free(cnx, stream_id, st);
            }
            picoquic_reset_stream(cnx, stream_id, 0);
            break;
        default:
            /* The client closing the connection is the normal end of a run;
             * picoquic frees remaining stream contexts' links with it. */
            break;
    }
    return 0;
}

static int server_loop_cb(picoquic_quic_t *quic, picoquic_packet_loop_cb_enum mode, void *ctx,
                          void *arg) {
    (void)quic;
    (void)ctx;
    switch (mode) {
        case picoquic_packet_loop_ready:
            /* Ask to be polled before each wait so SIGINT/SIGTERM are noticed. */
            ((picoquic_packet_loop_options_t *)arg)->do_time_check = 1;
            break;
        case picoquic_packet_loop_time_check: {
            packet_loop_time_check_arg_t *t = arg;
            if (t->delta_t > NQ_SIGNAL_CHECK_US) {
                t->delta_t = NQ_SIGNAL_CHECK_US;
            }
            break;
        }
        default:
            break;
    }
    return nq_stop ? PICOQUIC_NO_ERROR_TERMINATE_PACKET_LOOP : 0;
}

static int run_server(const struct nq_args *args) {
    char host[256];
    uint16_t port;
    struct sockaddr_storage addr;
    socklen_t addrlen;
    picoquic_quic_t *quic;

    if (nq_split_host_port(args->listen, host, sizeof(host), &port) != 0 ||
        nq_resolve(host, port, &addr, &addrlen) != 0) {
        fprintf(stderr, "malformed listen address: %s\n", args->listen);
        return 1;
    }

    quic = picoquic_create(64, args->cert, args->key, NULL, NQ_ALPN, server_callback, NULL, NULL,
                           NULL, NULL, picoquic_current_time(), NULL, NULL, NULL, 0);
    if (!quic) {
        fprintf(stderr, "cannot create QUIC context (check certificate/key)\n");
        return 1;
    }
    picoquic_set_default_idle_timeout(quic, NQ_IDLE_TIMEOUT_MS);

    printf("Listening on %s\n", args->listen);
    fflush(stdout);

    /* picoquic's packet loop binds the port on every address of the listen
     * address's family. */
    int ret = picoquic_packet_loop(quic, port, addr.ss_family, 0, 0, 0, server_loop_cb, NULL);
    picoquic_free(quic);
    /* A signal interrupting the loop's wait surfaces as an error: expected. */
    if (nq_stop || ret == 0 || ret == PICOQUIC_NO_ERROR_TERMINATE_PACKET_LOOP) {
        return 0;
    }
    fprintf(stderr, "packet loop failed: 0x%x\n", ret);
    return 1;
}

int main(int argc, char **argv) {
    struct nq_args args;
    struct sigaction sa;
    int rc = nq_parse_args(argc, argv, &args);
    if (rc != 0) {
        return rc;
    }

    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = on_signal;
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGTERM, &sa, NULL);

    return args.mode == NQ_CLIENT ? run_client(&args) : run_server(&args);
}
