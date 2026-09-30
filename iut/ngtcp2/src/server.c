#include "common.h"

#include <errno.h>
#include <poll.h>
#include <unistd.h>

#include <openssl/err.h>

/* Connection IDs a connection can be reached by (ours plus the client's ODCID). */
#define NQ_MAX_CIDS 16
/* Response bytes are zeros served from this buffer. ngtcp2 keeps pointers into
 * it until the data is acknowledged, which is fine since it never changes. */
#define NQ_ZERO_CHUNK (64 * 1024)
static const uint8_t zeros[NQ_ZERO_CHUNK];

struct stream {
    int64_t id;
    uint8_t request[NQ_REQUEST_LEN];
    size_t request_len;
    int responding;     /* request complete, response in progress */
    uint64_t remaining; /* response bytes not yet handed to ngtcp2 */
    int fin_sent;
    int blocked;        /* stream flow control exhausted in this write round */
    struct stream *next;
};

struct conn {
    ngtcp2_crypto_conn_ref conn_ref;
    ngtcp2_conn *conn;
    SSL *ssl;
    struct sockaddr_storage remote_addr;
    socklen_t remote_addrlen;
    ngtcp2_cid cids[NQ_MAX_CIDS];
    size_t ncids;
    struct stream *streams;
    ngtcp2_ccerr last_error;
    struct conn *next;
};

struct server {
    int fd;
    struct sockaddr_storage local_addr;
    socklen_t local_addrlen;
    SSL_CTX *ssl_ctx;
    struct conn *conns;
};

static ngtcp2_conn *get_conn(ngtcp2_crypto_conn_ref *ref) {
    return ((struct conn *)ref->user_data)->conn;
}

static void conn_add_cid(struct conn *c, const ngtcp2_cid *cid) {
    if (c->ncids < NQ_MAX_CIDS) {
        c->cids[c->ncids++] = *cid;
    }
}

static struct conn *find_conn(struct server *s, const uint8_t *dcid, size_t dcidlen) {
    for (struct conn *c = s->conns; c; c = c->next) {
        for (size_t i = 0; i < c->ncids; ++i) {
            if (c->cids[i].datalen == dcidlen && memcmp(c->cids[i].data, dcid, dcidlen) == 0) {
                return c;
            }
        }
    }
    return NULL;
}

static void conn_free(struct server *s, struct conn *c) {
    struct conn **p = &s->conns;
    while (*p && *p != c) {
        p = &(*p)->next;
    }
    if (*p) {
        *p = c->next;
    }
    while (c->streams) {
        struct stream *next = c->streams->next;
        free(c->streams);
        c->streams = next;
    }
    ngtcp2_conn_del(c->conn);
    SSL_free(c->ssl);
    free(c);
}

static struct stream *find_stream(struct conn *c, int64_t id) {
    for (struct stream *st = c->streams; st; st = st->next) {
        if (st->id == id) {
            return st;
        }
    }
    return NULL;
}

static int get_new_connection_id(ngtcp2_conn *conn, ngtcp2_cid *cid,
                                 ngtcp2_stateless_reset_token *token, size_t cidlen,
                                 void *user_data) {
    struct conn *c = user_data;
    if (c->ncids >= NQ_MAX_CIDS) {
        return NGTCP2_ERR_CALLBACK_FAILURE;
    }
    int rv = nq_get_new_connection_id_cb(conn, cid, token, cidlen, user_data);
    if (rv == 0) {
        conn_add_cid(c, cid);
    }
    return rv;
}

static int recv_stream_data(ngtcp2_conn *conn, uint32_t flags, int64_t stream_id,
                            uint64_t offset, const uint8_t *data, size_t datalen,
                            void *user_data, void *stream_user_data) {
    struct conn *c = user_data;
    struct stream *st = find_stream(c, stream_id);
    (void)offset;
    (void)stream_user_data;

    if (!st) {
        st = calloc(1, sizeof(*st));
        if (!st) {
            return NGTCP2_ERR_CALLBACK_FAILURE;
        }
        st->id = stream_id;
        st->next = c->streams;
        c->streams = st;
    }

    /* Only the leading 8 bytes are significant (docs/PROTOCOL.md). */
    for (size_t i = 0; i < datalen && st->request_len < NQ_REQUEST_LEN; ++i) {
        st->request[st->request_len++] = data[i];
    }
    ngtcp2_conn_extend_max_stream_offset(conn, stream_id, datalen);
    ngtcp2_conn_extend_max_offset(conn, datalen);

    if ((flags & NGTCP2_STREAM_DATA_FLAG_FIN) && !st->responding) {
        if (st->request_len < NQ_REQUEST_LEN) {
            ngtcp2_conn_shutdown_stream(conn, 0, stream_id, 0);
            return 0;
        }
        st->responding = 1;
        st->remaining = nq_request_decode(st->request);
    }
    return 0;
}

static int stream_close(ngtcp2_conn *conn, uint32_t flags, int64_t stream_id,
                        uint64_t app_error_code, void *user_data, void *stream_user_data) {
    struct conn *c = user_data;
    (void)conn;
    (void)flags;
    (void)app_error_code;
    (void)stream_user_data;

    for (struct stream **p = &c->streams; *p; p = &(*p)->next) {
        if ((*p)->id == stream_id) {
            struct stream *st = *p;
            *p = st->next;
            free(st);
            break;
        }
    }
    /* Let the client open another stream in place of the closed one. */
    if ((stream_id & 0x3) == 0) {
        ngtcp2_conn_extend_max_streams_bidi(conn, 1);
    }
    return 0;
}

static int extend_max_stream_data(ngtcp2_conn *conn, int64_t stream_id, uint64_t max_data,
                                  void *user_data, void *stream_user_data) {
    struct stream *st = find_stream(user_data, stream_id);
    (void)conn;
    (void)max_data;
    (void)stream_user_data;
    if (st) {
        st->blocked = 0;
    }
    return 0;
}

static struct stream *next_pending_stream(struct conn *c) {
    for (struct stream *st = c->streams; st; st = st->next) {
        if (st->responding && !st->fin_sent && !st->blocked) {
            return st;
        }
    }
    return NULL;
}

/* Writes as many packets as congestion control and pacing allow. */
static int conn_write(struct server *s, struct conn *c) {
    uint8_t buf[NQ_MAX_UDP_PAYLOAD];
    ngtcp2_path_storage ps;
    ngtcp2_pkt_info pi;
    ngtcp2_tstamp ts = nq_now_ns();
    size_t max_pkts = ngtcp2_conn_get_send_quantum2(c->conn) / NQ_MAX_UDP_PAYLOAD + 1;
    size_t pkts = 0;

    ngtcp2_path_storage_zero(&ps);
    for (struct stream *st = c->streams; st; st = st->next) {
        st->blocked = 0;
    }

    while (pkts < max_pkts) {
        struct stream *st = next_pending_stream(c);
        ngtcp2_vec vec = {0};
        size_t veccnt = 0;
        int64_t stream_id = -1;
        uint32_t flags = NGTCP2_WRITE_STREAM_FLAG_MORE;
        ngtcp2_ssize wdatalen = -1;

        if (st) {
            stream_id = st->id;
            vec.base = (uint8_t *)zeros;
            vec.len = st->remaining < NQ_ZERO_CHUNK ? (size_t)st->remaining : NQ_ZERO_CHUNK;
            veccnt = 1;
            if (st->remaining <= NQ_ZERO_CHUNK) {
                flags |= NGTCP2_WRITE_STREAM_FLAG_FIN;
            }
        }

        ngtcp2_ssize n = ngtcp2_conn_writev_stream(c->conn, &ps.path, &pi, buf, sizeof(buf),
                                                   &wdatalen, flags, stream_id, &vec, veccnt,
                                                   ts);
        if (st && wdatalen >= 0) {
            st->remaining -= (uint64_t)wdatalen;
            if (st->remaining == 0 && (flags & NGTCP2_WRITE_STREAM_FLAG_FIN)) {
                st->fin_sent = 1;
            }
        }
        if (n < 0) {
            if (n == NGTCP2_ERR_WRITE_MORE) {
                continue;
            }
            if (st && (n == NGTCP2_ERR_STREAM_DATA_BLOCKED || n == NGTCP2_ERR_STREAM_SHUT_WR ||
                       n == NGTCP2_ERR_STREAM_NOT_FOUND)) {
                st->blocked = 1;
                continue;
            }
            fprintf(stderr, "ngtcp2_conn_writev_stream: %s\n", ngtcp2_strerror((int)n));
            ngtcp2_ccerr_set_liberr(&c->last_error, (int)n, NULL, 0);
            return -1;
        }
        if (n == 0) {
            break;
        }
        if (nq_send_packet(s->fd, &ps.path, buf, (size_t)n) != 0) {
            return -1;
        }
        ++pkts;
    }

    ngtcp2_conn_update_pkt_tx_time(c->conn, ts);
    return 0;
}

static void conn_close(struct server *s, struct conn *c) {
    uint8_t buf[NQ_MAX_UDP_PAYLOAD];
    ngtcp2_path_storage ps;
    ngtcp2_pkt_info pi;

    if (ngtcp2_conn_in_closing_period2(c->conn) || ngtcp2_conn_in_draining_period2(c->conn)) {
        return;
    }
    ngtcp2_path_storage_zero(&ps);
    ngtcp2_ssize n = ngtcp2_conn_write_connection_close(c->conn, &ps.path, &pi, buf,
                                                        sizeof(buf), &c->last_error,
                                                        nq_now_ns());
    if (n > 0) {
        nq_send_packet(s->fd, &ps.path, buf, (size_t)n);
    }
}

static struct conn *accept_conn(struct server *s, const uint8_t *pkt, size_t len,
                                const ngtcp2_path *path) {
    ngtcp2_callbacks callbacks = {
        .recv_client_initial = ngtcp2_crypto_recv_client_initial_cb,
        .recv_crypto_data = ngtcp2_crypto_recv_crypto_data_cb,
        .encrypt = ngtcp2_crypto_encrypt_cb,
        .decrypt = ngtcp2_crypto_decrypt_cb,
        .hp_mask = ngtcp2_crypto_hp_mask_cb,
        .recv_stream_data = recv_stream_data,
        .stream_close = stream_close,
        .extend_max_stream_data = extend_max_stream_data,
        .rand = nq_rand_cb,
        .update_key = ngtcp2_crypto_update_key_cb,
        .delete_crypto_aead_ctx = ngtcp2_crypto_delete_crypto_aead_ctx_cb,
        .delete_crypto_cipher_ctx = ngtcp2_crypto_delete_crypto_cipher_ctx_cb,
        .version_negotiation = ngtcp2_crypto_version_negotiation_cb,
        .get_new_connection_id2 = get_new_connection_id,
        .get_path_challenge_data2 = ngtcp2_crypto_get_path_challenge_data2_cb,
    };
    ngtcp2_pkt_hd hd;
    ngtcp2_settings settings;
    ngtcp2_transport_params params;
    ngtcp2_cid scid;
    struct conn *c;
    int rv;

    if (ngtcp2_accept(&hd, pkt, len) != 0) {
        return NULL;
    }

    c = calloc(1, sizeof(*c));
    if (!c) {
        return NULL;
    }
    c->conn_ref.get_conn = get_conn;
    c->conn_ref.user_data = c;
    ngtcp2_ccerr_default(&c->last_error);
    memcpy(&c->remote_addr, path->remote.addr, path->remote.addrlen);
    c->remote_addrlen = path->remote.addrlen;

    scid.datalen = NQ_SCID_LEN;
    if (nq_random(scid.data, scid.datalen) != 0) {
        free(c);
        return NULL;
    }
    conn_add_cid(c, &scid);
    conn_add_cid(c, &hd.dcid);

    ngtcp2_settings_default(&settings);
    settings.initial_ts = nq_now_ns();
    settings.max_tx_udp_payload_size = NQ_MAX_UDP_PAYLOAD;

    ngtcp2_transport_params_default(&params);
    params.initial_max_streams_bidi = 100;
    params.initial_max_stream_data_bidi_remote = 64 * 1024;
    params.initial_max_data = 1024 * 1024;
    params.max_idle_timeout = NQ_IDLE_TIMEOUT;
    params.original_dcid = hd.dcid;
    params.original_dcid_present = 1;

    ngtcp2_path local_path = {
        .local = {.addr = (struct sockaddr *)&s->local_addr, .addrlen = s->local_addrlen},
        .remote = {.addr = (struct sockaddr *)&c->remote_addr, .addrlen = c->remote_addrlen},
    };
    rv = ngtcp2_conn_server_new(&c->conn, &hd.scid, &scid, &local_path, hd.version, &callbacks,
                                &settings, &params, NULL, c);
    if (rv != 0) {
        fprintf(stderr, "ngtcp2_conn_server_new: %s\n", ngtcp2_strerror(rv));
        free(c);
        return NULL;
    }

    c->ssl = SSL_new(s->ssl_ctx);
    if (!c->ssl) {
        fprintf(stderr, "SSL_new: %s\n", ERR_error_string(ERR_get_error(), NULL));
        ngtcp2_conn_del(c->conn);
        free(c);
        return NULL;
    }
    SSL_set_app_data(c->ssl, &c->conn_ref);
    SSL_set_accept_state(c->ssl);
    ngtcp2_conn_set_tls_native_handle(c->conn, c->ssl);

    c->next = s->conns;
    s->conns = c;
    return c;
}

/* Feeds one datagram to its connection. Returns -1 if the connection is done. */
static int handle_packet(struct server *s, const uint8_t *pkt, size_t len,
                         struct sockaddr_storage *from, socklen_t fromlen) {
    ngtcp2_version_cid vc;
    ngtcp2_pkt_info pi = {0};
    struct conn *c;
    int rv;

    rv = ngtcp2_pkt_decode_version_cid(&vc, pkt, len, NQ_SCID_LEN);
    if (rv != 0) {
        return 0;
    }

    ngtcp2_path path = {
        .local = {.addr = (struct sockaddr *)&s->local_addr, .addrlen = s->local_addrlen},
        .remote = {.addr = (struct sockaddr *)from, .addrlen = fromlen},
    };

    c = find_conn(s, vc.dcid, vc.dcidlen);
    if (!c) {
        c = accept_conn(s, pkt, len, &path);
        if (!c) {
            return 0;
        }
    }

    rv = ngtcp2_conn_read_pkt(c->conn, &path, &pi, pkt, len, nq_now_ns());
    if (rv != 0) {
        switch (rv) {
            case NGTCP2_ERR_DRAINING:
            case NGTCP2_ERR_CLOSING:
                /* The client closed the connection: the normal end of a run. */
                break;
            case NGTCP2_ERR_DROP_CONN:
                break;
            case NGTCP2_ERR_CRYPTO:
                fprintf(stderr, "ngtcp2_conn_read_pkt: %s\n", ngtcp2_strerror(rv));
                ngtcp2_ccerr_set_tls_alert(&c->last_error, ngtcp2_conn_get_tls_alert2(c->conn),
                                           NULL, 0);
                conn_close(s, c);
                break;
            default:
                fprintf(stderr, "ngtcp2_conn_read_pkt: %s\n", ngtcp2_strerror(rv));
                ngtcp2_ccerr_set_liberr(&c->last_error, rv, NULL, 0);
                conn_close(s, c);
                break;
        }
        conn_free(s, c);
        return -1;
    }

    if (conn_write(s, c) != 0) {
        conn_close(s, c);
        conn_free(s, c);
        return -1;
    }
    return 0;
}

static int server_read(struct server *s) {
    uint8_t buf[65536];
    for (;;) {
        struct sockaddr_storage from;
        socklen_t fromlen = sizeof(from);
        ssize_t n = recvfrom(s->fd, buf, sizeof(buf), MSG_DONTWAIT, (struct sockaddr *)&from,
                             &fromlen);
        if (n == -1) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                return 0;
            }
            fprintf(stderr, "recvfrom: %s\n", strerror(errno));
            return -1;
        }
        handle_packet(s, buf, (size_t)n, &from, fromlen);
    }
}

static void server_handle_timers(struct server *s) {
    ngtcp2_tstamp now = nq_now_ns();
    struct conn *c = s->conns;
    while (c) {
        struct conn *next = c->next;
        if (ngtcp2_conn_get_expiry2(c->conn) <= now) {
            int rv = ngtcp2_conn_handle_expiry(c->conn, now);
            if (rv != 0) {
                /* Idle timeout or closing/draining period over. */
                conn_free(s, c);
            } else if (conn_write(s, c) != 0) {
                conn_close(s, c);
                conn_free(s, c);
            }
        }
        c = next;
    }
}

static int server_timeout(struct server *s) {
    ngtcp2_tstamp expiry = UINT64_MAX;
    for (struct conn *c = s->conns; c; c = c->next) {
        ngtcp2_tstamp e = ngtcp2_conn_get_expiry2(c->conn);
        if (e < expiry) {
            expiry = e;
        }
    }
    return nq_poll_timeout(expiry, nq_now_ns());
}

int nq_run_server(const struct nq_args *args) {
    struct server s;
    char host[256];
    uint16_t port;
    int rc = 1;

    memset(&s, 0, sizeof(s));
    s.fd = -1;

    if (nq_split_host_port(args->listen, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed listen address: %s\n", args->listen);
        return 1;
    }
    s.ssl_ctx = nq_server_ssl_ctx(args->cert, args->key);
    if (!s.ssl_ctx) {
        return 1;
    }
    if (nq_resolve(host, port, &s.local_addr, &s.local_addrlen) != 0) {
        goto out;
    }
    s.fd = socket(s.local_addr.ss_family, SOCK_DGRAM, 0);
    if (s.fd == -1 || bind(s.fd, (struct sockaddr *)&s.local_addr, s.local_addrlen) != 0) {
        fprintf(stderr, "cannot bind %s: %s\n", args->listen, strerror(errno));
        goto out;
    }

    printf("Listening on %s\n", args->listen);
    fflush(stdout);

    while (!nq_stop) {
        struct pollfd pfd = {.fd = s.fd, .events = POLLIN};
        int n = poll(&pfd, 1, server_timeout(&s));
        if (nq_stop) {
            break;
        }
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            fprintf(stderr, "poll: %s\n", strerror(errno));
            goto out;
        }
        if (n > 0 && server_read(&s) != 0) {
            goto out;
        }
        server_handle_timers(&s);
    }
    rc = 0;

out:
    while (s.conns) {
        conn_free(&s, s.conns);
    }
    SSL_CTX_free(s.ssl_ctx);
    if (s.fd != -1) {
        close(s.fd);
    }
    return rc;
}
