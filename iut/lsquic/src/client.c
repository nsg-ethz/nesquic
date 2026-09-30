#include "common.h"

#include <errno.h>
#include <unistd.h>

#include <openssl/pem.h>
#include <openssl/x509.h>

struct client {
    const char *host;
    X509_STORE *trust;          /* holds only the --cert certificate */
    uint8_t request[NQ_REQUEST_LEN];
    size_t request_sent;
    uint64_t requested;
    uint64_t received;
    int fin;                    /* response fully received */
    int ok;
    int closed;                 /* connection closed: leave the event loop */
};

static struct client g_client;

static lsquic_conn_ctx_t *on_new_conn(void *ctx, lsquic_conn_t *conn) {
    (void)ctx;
    /* Queued until the handshake completes. */
    lsquic_conn_make_stream(conn);
    return NULL;
}

static void on_hsk_done(lsquic_conn_t *conn, enum lsquic_hsk_status status) {
    (void)conn;
    if (status != LSQ_HSK_OK && status != LSQ_HSK_RESUMED_OK) {
        fprintf(stderr, "handshake failed\n");
    }
}

static void on_conn_closed(lsquic_conn_t *conn) {
    (void)conn;
    g_client.closed = 1;
}

static lsquic_stream_ctx_t *on_new_stream(void *ctx, lsquic_stream_t *stream) {
    (void)ctx;
    if (stream) {
        lsquic_stream_wantwrite(stream, 1);
    }
    return NULL;
}

static void on_write(lsquic_stream_t *stream, lsquic_stream_ctx_t *h) {
    struct client *c = &g_client;
    (void)h;

    ssize_t n = lsquic_stream_write(stream, c->request + c->request_sent,
                                    NQ_REQUEST_LEN - c->request_sent);
    if (n < 0) {
        fprintf(stderr, "lsquic_stream_write: %s\n", strerror(errno));
        lsquic_conn_abort(lsquic_stream_conn(stream));
        return;
    }
    c->request_sent += (size_t)n;
    if (c->request_sent == NQ_REQUEST_LEN) {
        /* Finish the send side (FIN) and wait for the blob. */
        lsquic_stream_shutdown(stream, 1);
        lsquic_stream_wantwrite(stream, 0);
        lsquic_stream_wantread(stream, 1);
    }
}

static size_t discard(void *ctx, const unsigned char *buf, size_t len, int fin) {
    struct client *c = ctx;
    (void)buf;
    c->received += len;
    if (fin) {
        c->fin = 1;
    }
    return len;
}

static void on_read(lsquic_stream_t *stream, lsquic_stream_ctx_t *h) {
    struct client *c = &g_client;
    (void)h;

    ssize_t n = lsquic_stream_readf(stream, discard, c);
    if (n < 0) {
        fprintf(stderr, "lsquic_stream_readf: %s\n", strerror(errno));
        lsquic_conn_abort(lsquic_stream_conn(stream));
        return;
    }
    if (n == 0 || c->fin) {
        c->ok = c->received == c->requested;
        if (!c->ok) {
            fprintf(stderr, "received blob size (%lluB) different from requested (%lluB)\n",
                    (unsigned long long)c->received, (unsigned long long)c->requested);
        }
        lsquic_stream_wantread(stream, 0);
        /* Single exchange done: close the connection (application close). */
        lsquic_conn_close(lsquic_stream_conn(stream));
    }
}

static void on_close(lsquic_stream_t *stream, lsquic_stream_ctx_t *h) {
    (void)stream;
    (void)h;
}

static const struct lsquic_stream_if stream_if = {
    .on_new_conn = on_new_conn,
    .on_conn_closed = on_conn_closed,
    .on_new_stream = on_new_stream,
    .on_read = on_read,
    .on_write = on_write,
    .on_close = on_close,
    .on_hsk_done = on_hsk_done,
};

/* Validates the server chain against --cert and the URL host. */
static int verify_cert(void *ctx, STACK_OF(X509) *chain) {
    struct client *c = ctx;
    X509_STORE_CTX *store_ctx = X509_STORE_CTX_new();
    int ok = 0;

    if (store_ctx &&
        X509_STORE_CTX_init(store_ctx, c->trust, sk_X509_value(chain, 0), chain) == 1) {
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
    return ok ? 0 : -1;
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
    struct nq_socket sock = {.fd = -1};
    struct sockaddr_storage peer;
    socklen_t peerlen;
    struct lsquic_engine_settings settings;
    struct lsquic_engine_api api;
    lsquic_engine_t *engine = NULL;
    char host[256], err[256];
    uint16_t port;
    int rc = 1;

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

    sock.fd = socket(peer.ss_family, SOCK_DGRAM, 0);
    if (sock.fd == -1 || connect(sock.fd, (struct sockaddr *)&peer, peerlen) != 0) {
        fprintf(stderr, "cannot connect socket: %s\n", strerror(errno));
        goto out;
    }
    sock.local_addrlen = sizeof(sock.local_addr);
    getsockname(sock.fd, (struct sockaddr *)&sock.local_addr, &sock.local_addrlen);

    lsquic_engine_init_settings(&settings, 0);
    settings.es_versions = 1 << LSQVER_I001;
    settings.es_idle_timeout = NQ_IDLE_TIMEOUT_S;
    if (lsquic_engine_check_settings(&settings, 0, err, sizeof(err)) != 0) {
        fprintf(stderr, "invalid settings: %s\n", err);
        goto out;
    }

    memset(&api, 0, sizeof(api));
    api.ea_settings = &settings;
    api.ea_stream_if = &stream_if;
    api.ea_packets_out = nq_packets_out;
    api.ea_packets_out_ctx = &sock;
    api.ea_verify_cert = verify_cert;
    api.ea_verify_ctx = c;
    api.ea_alpn = NQ_ALPN;

    engine = lsquic_engine_new(0, &api);
    if (!engine) {
        fprintf(stderr, "lsquic_engine_new failed\n");
        goto out;
    }

    if (!lsquic_engine_connect(engine, LSQVER_I001, (struct sockaddr *)&sock.local_addr,
                               (struct sockaddr *)&peer, &sock, NULL,
                               nq_is_ip_literal(host) ? NULL : host, 0, NULL, 0, NULL, 0)) {
        fprintf(stderr, "lsquic_engine_connect failed\n");
        goto out;
    }

    if (nq_event_loop(engine, &sock, &c->closed) == 0 && !nq_stop) {
        rc = c->ok ? 0 : 1;
    }

out:
    if (engine) {
        lsquic_engine_destroy(engine);
    }
    X509_STORE_free(c->trust);
    if (sock.fd != -1) {
        close(sock.fd);
    }
    return rc;
}
