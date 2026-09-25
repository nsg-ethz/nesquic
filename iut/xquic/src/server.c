#include "common.h"

#include <errno.h>
#include <unistd.h>

/* Response bytes are zeros served from this buffer. */
#define NQ_ZERO_CHUNK (64 * 1024)
static unsigned char zeros[NQ_ZERO_CHUNK];

struct conn {
    xqc_cid_t cid;
    struct conn *prev, *next;
};

struct stream {
    uint8_t request[NQ_REQUEST_LEN];
    size_t request_len;
    int responding;     /* request complete, response in progress */
    uint64_t remaining; /* response bytes not yet accepted by xquic */
};

static struct conn *g_conns;

static int server_accept(xqc_engine_t *engine, xqc_connection_t *conn, const xqc_cid_t *cid,
                         void *user_data) {
    (void)engine;
    (void)user_data;
    struct conn *c = calloc(1, sizeof(*c));
    if (!c) {
        return -1;
    }
    memcpy(&c->cid, cid, sizeof(c->cid));
    c->next = g_conns;
    if (g_conns) {
        g_conns->prev = c;
    }
    g_conns = c;
    xqc_conn_set_transport_user_data(conn, c);
    return 0;
}

static void server_refuse(xqc_engine_t *engine, xqc_connection_t *conn, const xqc_cid_t *cid,
                          void *user_data) {
    (void)engine;
    (void)conn;
    (void)cid;
    struct conn *c = user_data;
    if (!c) {
        return;
    }
    if (c->prev) {
        c->prev->next = c->next;
    } else if (g_conns == c) {
        g_conns = c->next;
    }
    if (c->next) {
        c->next->prev = c->prev;
    }
    free(c);
}

static void update_cid(xqc_connection_t *conn, const xqc_cid_t *retire_cid,
                       const xqc_cid_t *new_cid, void *user_data) {
    struct conn *c = user_data;
    (void)conn;
    (void)retire_cid;
    if (c) {
        memcpy(&c->cid, new_cid, sizeof(c->cid));
    }
}

static int conn_create_notify(xqc_connection_t *conn, const xqc_cid_t *cid, void *user_data,
                              void *proto_data) {
    (void)conn;
    (void)cid;
    (void)user_data;
    (void)proto_data;
    return 0;
}

static int conn_close_notify(xqc_connection_t *conn, const xqc_cid_t *cid, void *user_data,
                             void *proto_data) {
    (void)proto_data;
    /* The client closing the connection is the normal end of a run. */
    server_refuse(NULL, conn, cid, user_data);
    return 0;
}

static void handshake_finished(xqc_connection_t *conn, void *user_data, void *proto_data) {
    (void)conn;
    (void)user_data;
    (void)proto_data;
}

static int send_response(xqc_stream_t *stream, struct stream *st) {
    while (st->remaining > 0) {
        size_t len = st->remaining < NQ_ZERO_CHUNK ? (size_t)st->remaining : NQ_ZERO_CHUNK;
        uint8_t fin = st->remaining == len;
        ssize_t n = xqc_stream_send(stream, zeros, len, fin);
        if (n == -XQC_EAGAIN) {
            /* Flow or congestion controlled; resumed from stream_write_notify. */
            return 0;
        }
        if (n < 0) {
            fprintf(stderr, "xqc_stream_send: %zd\n", n);
            return -1;
        }
        st->remaining -= (uint64_t)n;
        if (st->remaining > 0 && (size_t)n < len) {
            return 0;
        }
    }
    return 0;
}

static int stream_create_notify(xqc_stream_t *stream, void *user_data) {
    (void)user_data;
    struct stream *st = calloc(1, sizeof(*st));
    if (!st) {
        return -1;
    }
    xqc_stream_set_user_data(stream, st);
    return 0;
}

static int stream_read_notify(xqc_stream_t *stream, void *user_data) {
    struct stream *st = user_data;
    unsigned char buf[4096];
    uint8_t fin = 0;
    ssize_t n;

    do {
        n = xqc_stream_recv(stream, buf, sizeof(buf), &fin);
        if (n == -XQC_EAGAIN) {
            return 0;
        }
        if (n < 0) {
            return 0;
        }
        /* Only the leading 8 bytes are significant (docs/PROTOCOL.md). */
        for (ssize_t i = 0; i < n && st->request_len < NQ_REQUEST_LEN; ++i) {
            st->request[st->request_len++] = buf[i];
        }
    } while (n > 0 && !fin);

    if (fin && !st->responding) {
        if (st->request_len < NQ_REQUEST_LEN) {
            xqc_stream_close(stream);
            return 0;
        }
        st->responding = 1;
        st->remaining = nq_request_decode(st->request);
        if (st->remaining == 0) {
            xqc_stream_send(stream, zeros, 0, 1);
            return 0;
        }
        return send_response(stream, st);
    }
    return 0;
}

static int stream_write_notify(xqc_stream_t *stream, void *user_data) {
    struct stream *st = user_data;
    if (!st || !st->responding) {
        return 0;
    }
    return send_response(stream, st);
}

static int stream_close_notify(xqc_stream_t *stream, void *user_data) {
    (void)stream;
    free(user_data);
    return 0;
}

static void on_writable(xqc_engine_t *engine) {
    for (struct conn *c = g_conns; c; c = c->next) {
        xqc_conn_continue_send(engine, &c->cid);
    }
}

int nq_run_server(const struct nq_args *args) {
    xqc_engine_t *engine = NULL;
    xqc_config_t config;
    xqc_engine_ssl_config_t engine_ssl;
    xqc_conn_settings_t settings;
    char host[256];
    uint16_t port;
    int rc = 1;

    xqc_engine_callback_t engine_cbs = {
        .set_event_timer = nq_set_event_timer,
        .log_callbacks = {.xqc_log_write_err = nq_log_write},
    };
    xqc_transport_callbacks_t transport_cbs = {
        .server_accept = server_accept,
        .server_refuse = server_refuse,
        .write_socket = nq_write_socket,
        .conn_update_cid_notify = update_cid,
        .conn_send_packet_before_accept = nq_write_socket,
    };
    xqc_app_proto_callbacks_t app_cbs = {
        .conn_cbs = {
            .conn_create_notify = conn_create_notify,
            .conn_close_notify = conn_close_notify,
            .conn_handshake_finished = handshake_finished,
        },
        .stream_cbs = {
            .stream_read_notify = stream_read_notify,
            .stream_write_notify = stream_write_notify,
            .stream_create_notify = stream_create_notify,
            .stream_close_notify = stream_close_notify,
        },
    };

    if (nq_split_host_port(args->listen, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed listen address: %s\n", args->listen);
        return 1;
    }
    if (nq_resolve(host, port, &nq_loop.local_addr, &nq_loop.local_addrlen) != 0) {
        return 1;
    }
    nq_loop.fd = socket(nq_loop.local_addr.ss_family, SOCK_DGRAM, 0);
    if (nq_loop.fd == -1 ||
        bind(nq_loop.fd, (struct sockaddr *)&nq_loop.local_addr, nq_loop.local_addrlen) != 0) {
        fprintf(stderr, "cannot bind %s: %s\n", args->listen, strerror(errno));
        goto out;
    }

    if (xqc_engine_get_default_config(&config, XQC_ENGINE_SERVER) != XQC_OK) {
        goto out;
    }
    config.cfg_log_level = XQC_LOG_ERROR;

    memset(&engine_ssl, 0, sizeof(engine_ssl));
    engine_ssl.private_key_file = (char *)args->key;
    engine_ssl.cert_file = (char *)args->cert;
    engine_ssl.ciphers = XQC_TLS_CIPHERS;
    engine_ssl.groups = XQC_TLS_GROUPS;

    engine = xqc_engine_create(XQC_ENGINE_SERVER, &config, &engine_ssl, &engine_cbs,
                               &transport_cbs, NULL);
    if (!engine) {
        fprintf(stderr, "xqc_engine_create failed\n");
        goto out;
    }
    if (xqc_engine_register_alpn(engine, NQ_ALPN, strlen(NQ_ALPN), &app_cbs, NULL) != XQC_OK) {
        fprintf(stderr, "xqc_engine_register_alpn failed\n");
        goto out;
    }
    nq_conn_settings(&settings);
    xqc_server_set_conn_settings(engine, &settings);

    printf("Listening on %s\n", args->listen);
    fflush(stdout);

    rc = nq_event_loop(engine, NULL, on_writable) == 0 ? 0 : 1;

out:
    if (engine) {
        xqc_engine_destroy(engine);
    }
    while (g_conns) {
        struct conn *next = g_conns->next;
        free(g_conns);
        g_conns = next;
    }
    if (nq_loop.fd != -1) {
        close(nq_loop.fd);
    }
    return rc;
}
