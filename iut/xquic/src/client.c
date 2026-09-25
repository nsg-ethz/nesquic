#include "common.h"

#include <errno.h>
#include <unistd.h>

#include <openssl/ssl.h>
#include <openssl/x509.h>

struct client {
    xqc_engine_t *engine;
    xqc_cid_t cid;
    const char *host;
    X509_STORE *trust;          /* holds only the --cert certificate */
    uint8_t request[NQ_REQUEST_LEN];
    uint64_t requested;
    uint64_t received;
    int ok;
    int closed;                 /* connection closed: leave the event loop */
};

static struct client g_client;

/* Validates the server chain against --cert and the URL host. */
static int verify_peer(struct client *c, SSL *ssl) {
    X509 *leaf = ssl ? SSL_get_peer_certificate(ssl) : NULL;
    X509_STORE_CTX *store_ctx = X509_STORE_CTX_new();
    int ok = 0;

    if (leaf && store_ctx &&
        X509_STORE_CTX_init(store_ctx, c->trust, leaf, SSL_get_peer_cert_chain(ssl)) == 1) {
        X509_VERIFY_PARAM *param = X509_STORE_CTX_get0_param(store_ctx);
        if (nq_is_ip_literal(c->host)) {
            X509_VERIFY_PARAM_set1_ip_asc(param, c->host);
        } else {
            X509_VERIFY_PARAM_set1_host(param, c->host, strlen(c->host));
        }
        ok = X509_verify_cert(store_ctx) == 1;
        if (!ok) {
            fprintf(stderr, "certificate verification failed: %s\n",
                    X509_verify_cert_error_string(X509_STORE_CTX_get_error(store_ctx)));
        }
    }
    X509_STORE_CTX_free(store_ctx);
    X509_free(leaf);
    return ok;
}

static void handshake_finished(xqc_connection_t *conn, void *user_data, void *proto_data) {
    struct client *c = user_data;
    (void)proto_data;

    /* xquic's built-in check cannot match IP SANs, so verify here before any
     * application data is sent. */
    if (!verify_peer(c, xqc_conn_get_ssl(conn))) {
        xqc_conn_close(c->engine, &c->cid);
        return;
    }

    xqc_stream_settings_t stream_settings = {.recv_rate_bytes_per_sec = 1};
    xqc_stream_t *stream = xqc_stream_create(c->engine, &c->cid, &stream_settings, c);
    if (!stream) {
        fprintf(stderr, "xqc_stream_create failed\n");
        xqc_conn_close(c->engine, &c->cid);
        return;
    }
    /* The 8-byte request always fits the initial flow control window. */
    if (xqc_stream_send(stream, c->request, NQ_REQUEST_LEN, 1) != NQ_REQUEST_LEN) {
        fprintf(stderr, "xqc_stream_send failed\n");
        xqc_conn_close(c->engine, &c->cid);
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
    struct client *c = user_data;
    (void)cid;
    (void)proto_data;
    int err = xqc_conn_get_errno(conn);
    if (err != 0 && !c->ok) {
        fprintf(stderr, "connection closed with error %d\n", err);
    }
    c->closed = 1;
    return 0;
}

static int stream_read_notify(xqc_stream_t *stream, void *user_data) {
    struct client *c = user_data;
    unsigned char buf[65536];
    uint8_t fin = 0;
    ssize_t n;

    do {
        n = xqc_stream_recv(stream, buf, sizeof(buf), &fin);
        if (n == -XQC_EAGAIN) {
            return 0;
        }
        if (n < 0) {
            fprintf(stderr, "xqc_stream_recv: %zd\n", n);
            xqc_conn_close(c->engine, &c->cid);
            return 0;
        }
        c->received += (uint64_t)n;
    } while (n > 0 && !fin);

    if (fin) {
        c->ok = c->received == c->requested;
        if (!c->ok) {
            fprintf(stderr, "received blob size (%lluB) different from requested (%lluB)\n",
                    (unsigned long long)c->received, (unsigned long long)c->requested);
        }
        /* Single exchange done: close the connection (application close). */
        xqc_conn_close(c->engine, &c->cid);
    }
    return 0;
}

static int stream_noop_notify(xqc_stream_t *stream, void *user_data) {
    (void)stream;
    (void)user_data;
    return 0;
}

/* xquic calls these unconditionally on clients; there is no resumption here. */
static void save_token(const unsigned char *token, uint32_t len, void *user_data) {
    (void)token;
    (void)len;
    (void)user_data;
}

static void save_data(const char *data, size_t len, void *user_data) {
    (void)data;
    (void)len;
    (void)user_data;
}

static void on_writable(xqc_engine_t *engine) {
    xqc_conn_continue_send(engine, &g_client.cid);
}

static X509_STORE *load_trust(const char *path) {
    X509_STORE *store = X509_STORE_new();
    if (!store || X509_STORE_load_locations(store, path, NULL) != 1) {
        fprintf(stderr, "cannot load CA %s\n", path);
        X509_STORE_free(store);
        return NULL;
    }
    return store;
}

int nq_run_client(const struct nq_args *args) {
    struct client *c = &g_client;
    struct sockaddr_storage peer;
    socklen_t peerlen;
    xqc_config_t config;
    xqc_engine_ssl_config_t engine_ssl;
    xqc_conn_settings_t settings;
    xqc_conn_ssl_config_t conn_ssl;
    char host[256];
    uint16_t port;
    int rc = 1;

    xqc_engine_callback_t engine_cbs = {
        .set_event_timer = nq_set_event_timer,
        .log_callbacks = {.xqc_log_write_err = nq_log_write},
    };
    xqc_transport_callbacks_t transport_cbs = {
        .write_socket = nq_write_socket,
        .save_token = save_token,
        .save_session_cb = save_data,
        .save_tp_cb = save_data,
    };
    xqc_app_proto_callbacks_t app_cbs = {
        .conn_cbs = {
            .conn_create_notify = conn_create_notify,
            .conn_close_notify = conn_close_notify,
            .conn_handshake_finished = handshake_finished,
        },
        .stream_cbs = {
            .stream_read_notify = stream_read_notify,
            .stream_write_notify = stream_noop_notify,
            .stream_create_notify = stream_noop_notify,
            .stream_close_notify = stream_noop_notify,
        },
    };

    if (nq_blob_bytes(args->blob, &c->requested) != 0) {
        fprintf(stderr, "malformed blob size: %s\n", args->blob);
        return 1;
    }
    nq_request_encode(c->requested, c->request);

    if (nq_split_host_port(args->url, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed url: %s\n", args->url);
        return 1;
    }
    c->host = host;
    c->trust = load_trust(args->cert);
    if (!c->trust || nq_resolve(host, port, &peer, &peerlen) != 0) {
        goto out;
    }

    nq_loop.fd = socket(peer.ss_family, SOCK_DGRAM, 0);
    if (nq_loop.fd == -1 || connect(nq_loop.fd, (struct sockaddr *)&peer, peerlen) != 0) {
        fprintf(stderr, "cannot connect socket: %s\n", strerror(errno));
        goto out;
    }
    nq_loop.local_addrlen = sizeof(nq_loop.local_addr);
    getsockname(nq_loop.fd, (struct sockaddr *)&nq_loop.local_addr, &nq_loop.local_addrlen);

    if (xqc_engine_get_default_config(&config, XQC_ENGINE_CLIENT) != XQC_OK) {
        goto out;
    }
    config.cfg_log_level = XQC_LOG_ERROR;

    memset(&engine_ssl, 0, sizeof(engine_ssl));
    engine_ssl.ciphers = XQC_TLS_CIPHERS;
    engine_ssl.groups = XQC_TLS_GROUPS;

    c->engine = xqc_engine_create(XQC_ENGINE_CLIENT, &config, &engine_ssl, &engine_cbs,
                                  &transport_cbs, c);
    if (!c->engine) {
        fprintf(stderr, "xqc_engine_create failed\n");
        goto out;
    }
    if (xqc_engine_register_alpn(c->engine, NQ_ALPN, strlen(NQ_ALPN), &app_cbs, NULL) !=
        XQC_OK) {
        fprintf(stderr, "xqc_engine_register_alpn failed\n");
        goto out;
    }

    nq_conn_settings(&settings);
    memset(&conn_ssl, 0, sizeof(conn_ssl));

    const xqc_cid_t *cid = xqc_connect(c->engine, &settings, NULL, 0, host, 0, &conn_ssl,
                                       (struct sockaddr *)&peer, peerlen, NQ_ALPN, c);
    if (!cid) {
        fprintf(stderr, "xqc_connect failed\n");
        goto out;
    }
    memcpy(&c->cid, cid, sizeof(c->cid));

    if (nq_event_loop(c->engine, &c->closed, on_writable) == 0 && !nq_stop) {
        rc = c->ok ? 0 : 1;
    }

out:
    if (c->engine) {
        xqc_engine_destroy(c->engine);
    }
    X509_STORE_free(c->trust);
    if (nq_loop.fd != -1) {
        close(nq_loop.fd);
    }
    return rc;
}
