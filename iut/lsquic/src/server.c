#include "common.h"

#include <errno.h>
#include <unistd.h>

#include <openssl/err.h>
#include <openssl/ssl.h>

/* Per-stream state: accumulates the request, then counts down the response. */
struct lsquic_stream_ctx {
    uint8_t request[NQ_REQUEST_LEN];
    size_t request_len;
    uint64_t remaining;
};

static SSL_CTX *g_ssl_ctx;

static lsquic_conn_ctx_t *on_new_conn(void *ctx, lsquic_conn_t *conn) {
    (void)ctx;
    (void)conn;
    return NULL;
}

static void on_conn_closed(lsquic_conn_t *conn) {
    (void)conn;
}

static lsquic_stream_ctx_t *on_new_stream(void *ctx, lsquic_stream_t *stream) {
    (void)ctx;
    lsquic_stream_ctx_t *st = calloc(1, sizeof(*st));
    if (!st) {
        lsquic_conn_abort(lsquic_stream_conn(stream));
        return NULL;
    }
    lsquic_stream_wantread(stream, 1);
    return st;
}

static size_t collect(void *ctx, const unsigned char *buf, size_t len, int fin) {
    lsquic_stream_ctx_t *st = ctx;
    (void)fin;
    /* Only the leading 8 bytes are significant (docs/PROTOCOL.md). */
    for (size_t i = 0; i < len && st->request_len < NQ_REQUEST_LEN; ++i) {
        st->request[st->request_len++] = buf[i];
    }
    return len;
}

static void on_read(lsquic_stream_t *stream, lsquic_stream_ctx_t *st) {
    ssize_t n = lsquic_stream_readf(stream, collect, st);
    if (n < 0) {
        if (errno != EWOULDBLOCK) {
            lsquic_stream_close(stream);
        }
        return;
    }
    if (n > 0) {
        return;
    }

    /* End of request: serve the blob. */
    lsquic_stream_wantread(stream, 0);
    if (st->request_len < NQ_REQUEST_LEN) {
        lsquic_stream_close(stream);
        return;
    }
    st->remaining = nq_request_decode(st->request);
    if (st->remaining == 0) {
        lsquic_stream_shutdown(stream, 1);
    } else {
        lsquic_stream_wantwrite(stream, 1);
    }
}

static size_t zeros_size(void *ctx) {
    lsquic_stream_ctx_t *st = ctx;
    return st->remaining > SIZE_MAX ? SIZE_MAX : (size_t)st->remaining;
}

static size_t zeros_read(void *ctx, void *buf, size_t count) {
    lsquic_stream_ctx_t *st = ctx;
    if (count > st->remaining) {
        count = (size_t)st->remaining;
    }
    memset(buf, 0, count);
    st->remaining -= count;
    return count;
}

static void on_write(lsquic_stream_t *stream, lsquic_stream_ctx_t *st) {
    struct lsquic_reader reader = {zeros_read, zeros_size, st};

    if (lsquic_stream_writef(stream, &reader) < 0) {
        fprintf(stderr, "lsquic_stream_writef: %s\n", strerror(errno));
        lsquic_stream_close(stream);
        return;
    }
    if (st->remaining == 0) {
        lsquic_stream_wantwrite(stream, 0);
        lsquic_stream_shutdown(stream, 1);
    }
}

static void on_close(lsquic_stream_t *stream, lsquic_stream_ctx_t *st) {
    (void)stream;
    free(st);
}

static const struct lsquic_stream_if stream_if = {
    .on_new_conn = on_new_conn,
    .on_conn_closed = on_conn_closed,
    .on_new_stream = on_new_stream,
    .on_read = on_read,
    .on_write = on_write,
    .on_close = on_close,
};

static int alpn_select_cb(SSL *ssl, const unsigned char **out, unsigned char *outlen,
                          const unsigned char *in, unsigned int inlen, void *arg) {
    (void)ssl;
    (void)arg;
    unsigned int i = 0;
    while (i < inlen) {
        unsigned int len = in[i];
        if (i + 1 + len > inlen) {
            break;
        }
        if (len == strlen(NQ_ALPN) && memcmp(in + i + 1, NQ_ALPN, len) == 0) {
            *out = in + i + 1;
            *outlen = (unsigned char)len;
            return SSL_TLSEXT_ERR_OK;
        }
        i += 1 + len;
    }
    return SSL_TLSEXT_ERR_ALERT_FATAL;
}

static SSL_CTX *get_ssl_ctx(void *peer_ctx, const struct sockaddr *local) {
    (void)peer_ctx;
    (void)local;
    return g_ssl_ctx;
}

static SSL_CTX *lookup_cert(void *ctx, const struct sockaddr *local, const char *sni) {
    (void)ctx;
    (void)local;
    (void)sni;
    return g_ssl_ctx;
}

static SSL_CTX *make_ssl_ctx(const char *cert, const char *key) {
    SSL_CTX *ctx = SSL_CTX_new(TLS_method());
    if (!ctx) {
        return NULL;
    }
    SSL_CTX_set_min_proto_version(ctx, TLS1_3_VERSION);
    SSL_CTX_set_max_proto_version(ctx, TLS1_3_VERSION);
    SSL_CTX_set_alpn_select_cb(ctx, alpn_select_cb, NULL);
    if (SSL_CTX_use_certificate_chain_file(ctx, cert) != 1 ||
        SSL_CTX_use_PrivateKey_file(ctx, key, SSL_FILETYPE_PEM) != 1 ||
        SSL_CTX_check_private_key(ctx) != 1) {
        fprintf(stderr, "cannot load certificate/key: %s\n",
                ERR_error_string(ERR_get_error(), NULL));
        SSL_CTX_free(ctx);
        return NULL;
    }
    return ctx;
}

int nq_run_server(const struct nq_args *args) {
    struct nq_socket sock = {.fd = -1};
    struct lsquic_engine_settings settings;
    struct lsquic_engine_api api;
    lsquic_engine_t *engine = NULL;
    char host[256], err[256];
    uint16_t port;
    int rc = 1;

    if (nq_split_host_port(args->listen, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed listen address: %s\n", args->listen);
        return 1;
    }
    g_ssl_ctx = make_ssl_ctx(args->cert, args->key);
    if (!g_ssl_ctx || nq_resolve(host, port, &sock.local_addr, &sock.local_addrlen) != 0) {
        goto out;
    }
    sock.fd = socket(sock.local_addr.ss_family, SOCK_DGRAM, 0);
    if (sock.fd == -1 ||
        bind(sock.fd, (struct sockaddr *)&sock.local_addr, sock.local_addrlen) != 0) {
        fprintf(stderr, "cannot bind %s: %s\n", args->listen, strerror(errno));
        goto out;
    }

    lsquic_engine_init_settings(&settings, LSENG_SERVER);
    settings.es_versions = 1 << LSQVER_I001;
    settings.es_idle_timeout = NQ_IDLE_TIMEOUT_S;
    if (lsquic_engine_check_settings(&settings, LSENG_SERVER, err, sizeof(err)) != 0) {
        fprintf(stderr, "invalid settings: %s\n", err);
        goto out;
    }

    memset(&api, 0, sizeof(api));
    api.ea_settings = &settings;
    api.ea_stream_if = &stream_if;
    api.ea_packets_out = nq_packets_out;
    api.ea_packets_out_ctx = &sock;
    api.ea_get_ssl_ctx = get_ssl_ctx;
    api.ea_lookup_cert = lookup_cert;

    engine = lsquic_engine_new(LSENG_SERVER, &api);
    if (!engine) {
        fprintf(stderr, "lsquic_engine_new failed\n");
        goto out;
    }

    printf("Listening on %s\n", args->listen);
    fflush(stdout);

    rc = nq_event_loop(engine, &sock, NULL) == 0 ? 0 : 1;

out:
    if (engine) {
        lsquic_engine_destroy(engine);
    }
    SSL_CTX_free(g_ssl_ctx);
    if (sock.fd != -1) {
        close(sock.fd);
    }
    return rc;
}
