#include "common.h"

#include <errno.h>
#include <poll.h>
#include <unistd.h>

#include <openssl/err.h>
#include <openssl/x509.h>

struct client {
    ngtcp2_crypto_conn_ref conn_ref;
    int fd;
    struct sockaddr_storage local_addr, remote_addr;
    socklen_t local_addrlen, remote_addrlen;
    SSL_CTX *ssl_ctx;
    SSL *ssl;
    ngtcp2_conn *conn;
    ngtcp2_ccerr last_error;

    int64_t stream_id;              /* -1 until the request stream is open */
    uint8_t request[NQ_REQUEST_LEN];
    size_t request_sent;            /* request bytes accepted by ngtcp2 */
    uint64_t requested;             /* bytes expected in the response */
    uint64_t received;              /* bytes received so far */
    int done;                       /* response fully received */
    int ok;                         /* response had the requested length */
};

static ngtcp2_conn *get_conn(ngtcp2_crypto_conn_ref *ref) {
    return ((struct client *)ref->user_data)->conn;
}

static int extend_max_local_streams_bidi(ngtcp2_conn *conn, uint64_t max_streams,
                                         void *user_data) {
    struct client *c = user_data;
    (void)max_streams;

    if (c->stream_id != -1) {
        return 0;
    }
    /* Open the request stream as soon as the server allows it. */
    if (ngtcp2_conn_open_bidi_stream(conn, &c->stream_id, NULL) != 0) {
        c->stream_id = -1;
    }
    return 0;
}

static int recv_stream_data(ngtcp2_conn *conn, uint32_t flags, int64_t stream_id,
                            uint64_t offset, const uint8_t *data, size_t datalen,
                            void *user_data, void *stream_user_data) {
    struct client *c = user_data;
    (void)offset;
    (void)data;
    (void)stream_user_data;

    c->received += datalen;
    /* The data is consumed immediately, so hand the credit straight back. */
    ngtcp2_conn_extend_max_stream_offset(conn, stream_id, datalen);
    ngtcp2_conn_extend_max_offset(conn, datalen);

    if (flags & NGTCP2_STREAM_DATA_FLAG_FIN) {
        c->done = 1;
        c->ok = c->received == c->requested;
        if (!c->ok) {
            fprintf(stderr, "received blob size (%lluB) different from requested (%lluB)\n",
                    (unsigned long long)c->received, (unsigned long long)c->requested);
        }
    }
    return 0;
}

static int client_quic_init(struct client *c) {
    ngtcp2_path path = {
        .local = {.addr = (struct sockaddr *)&c->local_addr, .addrlen = c->local_addrlen},
        .remote = {.addr = (struct sockaddr *)&c->remote_addr, .addrlen = c->remote_addrlen},
    };
    ngtcp2_callbacks callbacks = {
        .client_initial = ngtcp2_crypto_client_initial_cb,
        .recv_crypto_data = ngtcp2_crypto_recv_crypto_data_cb,
        .encrypt = ngtcp2_crypto_encrypt_cb,
        .decrypt = ngtcp2_crypto_decrypt_cb,
        .hp_mask = ngtcp2_crypto_hp_mask_cb,
        .recv_retry = ngtcp2_crypto_recv_retry_cb,
        .recv_stream_data = recv_stream_data,
        .extend_max_local_streams_bidi = extend_max_local_streams_bidi,
        .rand = nq_rand_cb,
        .update_key = ngtcp2_crypto_update_key_cb,
        .delete_crypto_aead_ctx = ngtcp2_crypto_delete_crypto_aead_ctx_cb,
        .delete_crypto_cipher_ctx = ngtcp2_crypto_delete_crypto_cipher_ctx_cb,
        .version_negotiation = ngtcp2_crypto_version_negotiation_cb,
        .get_new_connection_id2 = nq_get_new_connection_id_cb,
        .get_path_challenge_data2 = ngtcp2_crypto_get_path_challenge_data2_cb,
    };
    ngtcp2_cid dcid, scid;
    ngtcp2_settings settings;
    ngtcp2_transport_params params;
    int rv;

    dcid.datalen = NGTCP2_MIN_INITIAL_DCIDLEN;
    scid.datalen = NQ_SCID_LEN;
    if (nq_random(dcid.data, dcid.datalen) != 0 || nq_random(scid.data, scid.datalen) != 0) {
        return -1;
    }

    ngtcp2_settings_default(&settings);
    settings.initial_ts = nq_now_ns();
    settings.max_tx_udp_payload_size = NQ_MAX_UDP_PAYLOAD;

    ngtcp2_transport_params_default(&params);
    params.initial_max_stream_data_bidi_local = 8 * 1024 * 1024;
    params.initial_max_data = 16 * 1024 * 1024;
    params.max_idle_timeout = NQ_IDLE_TIMEOUT;

    rv = ngtcp2_conn_client_new(&c->conn, &dcid, &scid, &path, NGTCP2_PROTO_VER_V1,
                                &callbacks, &settings, &params, NULL, c);
    if (rv != 0) {
        fprintf(stderr, "ngtcp2_conn_client_new: %s\n", ngtcp2_strerror(rv));
        return -1;
    }
    ngtcp2_conn_set_tls_native_handle(c->conn, c->ssl);
    return 0;
}

static int client_ssl_init(struct client *c, const char *ca_file, const char *host) {
    c->ssl_ctx = nq_client_ssl_ctx(ca_file);
    if (!c->ssl_ctx) {
        return -1;
    }
    c->ssl = SSL_new(c->ssl_ctx);
    if (!c->ssl) {
        fprintf(stderr, "SSL_new: %s\n", ERR_error_string(ERR_get_error(), NULL));
        return -1;
    }
    SSL_set_app_data(c->ssl, &c->conn_ref);
    SSL_set_connect_state(c->ssl);
    SSL_set_alpn_protos(c->ssl, (const uint8_t *)NQ_ALPN_WIRE, sizeof(NQ_ALPN_WIRE) - 1);

    /* Validate the server certificate against the URL host. */
    X509_VERIFY_PARAM *param = SSL_get0_param(c->ssl);
    if (nq_is_ip_literal(host)) {
        X509_VERIFY_PARAM_set1_ip_asc(param, host);
    } else {
        SSL_set_tlsext_host_name(c->ssl, host);
        X509_VERIFY_PARAM_set1_host(param, host, strlen(host));
    }
    return 0;
}

static int client_read(struct client *c) {
    uint8_t buf[65536];
    struct sockaddr_storage addr;
    ngtcp2_pkt_info pi = {0};

    for (;;) {
        socklen_t addrlen = sizeof(addr);
        ssize_t n = recvfrom(c->fd, buf, sizeof(buf), MSG_DONTWAIT, (struct sockaddr *)&addr,
                             &addrlen);
        if (n == -1) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                return 0;
            }
            fprintf(stderr, "recvfrom: %s\n", strerror(errno));
            return -1;
        }

        ngtcp2_path path = {
            .local = {.addr = (struct sockaddr *)&c->local_addr, .addrlen = c->local_addrlen},
            .remote = {.addr = (struct sockaddr *)&addr, .addrlen = addrlen},
        };
        int rv = ngtcp2_conn_read_pkt(c->conn, &path, &pi, buf, (size_t)n, nq_now_ns());
        if (rv != 0) {
            fprintf(stderr, "ngtcp2_conn_read_pkt: %s\n", ngtcp2_strerror(rv));
            if (rv == NGTCP2_ERR_CRYPTO) {
                ngtcp2_ccerr_set_tls_alert(&c->last_error, ngtcp2_conn_get_tls_alert2(c->conn),
                                           NULL, 0);
            } else {
                ngtcp2_ccerr_set_liberr(&c->last_error, rv, NULL, 0);
            }
            return -1;
        }
    }
}

static int client_write(struct client *c) {
    uint8_t buf[NQ_MAX_UDP_PAYLOAD];
    ngtcp2_path_storage ps;
    ngtcp2_pkt_info pi;
    ngtcp2_tstamp ts = nq_now_ns();
    size_t max_pkts = ngtcp2_conn_get_send_quantum2(c->conn) / NQ_MAX_UDP_PAYLOAD + 1;
    size_t pkts = 0;

    ngtcp2_path_storage_zero(&ps);

    while (pkts < max_pkts) {
        ngtcp2_vec vec = {0};
        size_t veccnt = 0;
        int64_t stream_id = -1;
        uint32_t flags = NGTCP2_WRITE_STREAM_FLAG_MORE;
        ngtcp2_ssize wdatalen = -1;

        if (c->stream_id != -1 && c->request_sent < NQ_REQUEST_LEN) {
            stream_id = c->stream_id;
            vec.base = c->request + c->request_sent;
            vec.len = NQ_REQUEST_LEN - c->request_sent;
            veccnt = 1;
            flags |= NGTCP2_WRITE_STREAM_FLAG_FIN;
        }

        ngtcp2_ssize n = ngtcp2_conn_writev_stream(c->conn, &ps.path, &pi, buf, sizeof(buf),
                                                   &wdatalen, flags, stream_id, &vec, veccnt,
                                                   ts);
        if (n < 0) {
            if (n == NGTCP2_ERR_WRITE_MORE) {
                c->request_sent += (size_t)wdatalen;
                continue;
            }
            fprintf(stderr, "ngtcp2_conn_writev_stream: %s\n", ngtcp2_strerror((int)n));
            ngtcp2_ccerr_set_liberr(&c->last_error, (int)n, NULL, 0);
            return -1;
        }
        if (wdatalen > 0) {
            c->request_sent += (size_t)wdatalen;
        }
        if (n == 0) {
            break;
        }
        if (nq_send_packet(c->fd, &ps.path, buf, (size_t)n) != 0) {
            return -1;
        }
        ++pkts;
    }

    ngtcp2_conn_update_pkt_tx_time(c->conn, ts);
    return 0;
}

/* Sends CONNECTION_CLOSE with the current error (application close on success). */
static void client_close(struct client *c) {
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
        nq_send_packet(c->fd, &ps.path, buf, (size_t)n);
    }
}

static int client_connect_socket(struct client *c, const char *host, uint16_t port) {
    if (nq_resolve(host, port, &c->remote_addr, &c->remote_addrlen) != 0) {
        return -1;
    }
    c->fd = socket(c->remote_addr.ss_family, SOCK_DGRAM, 0);
    if (c->fd == -1) {
        fprintf(stderr, "socket: %s\n", strerror(errno));
        return -1;
    }
    if (connect(c->fd, (struct sockaddr *)&c->remote_addr, c->remote_addrlen) != 0) {
        fprintf(stderr, "connect: %s\n", strerror(errno));
        return -1;
    }
    c->local_addrlen = sizeof(c->local_addr);
    if (getsockname(c->fd, (struct sockaddr *)&c->local_addr, &c->local_addrlen) != 0) {
        fprintf(stderr, "getsockname: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

int nq_run_client(const struct nq_args *args) {
    struct client c;
    char host[256];
    uint16_t port;
    int rc = 1;

    memset(&c, 0, sizeof(c));
    c.fd = -1;
    c.stream_id = -1;
    c.conn_ref.get_conn = get_conn;
    c.conn_ref.user_data = &c;
    ngtcp2_ccerr_default(&c.last_error);

    if (nq_blob_bytes(args->blob, &c.requested) != 0) {
        fprintf(stderr, "malformed blob size: %s\n", args->blob);
        return 1;
    }
    nq_request_encode(c.requested, c.request);

    if (nq_split_host_port(args->url, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed url: %s\n", args->url);
        return 1;
    }
    if (client_connect_socket(&c, host, port) != 0 ||
        client_ssl_init(&c, args->cert, host) != 0 || client_quic_init(&c) != 0) {
        goto out;
    }

    for (;;) {
        if (client_write(&c) != 0) {
            client_close(&c);
            goto out;
        }

        struct pollfd pfd = {.fd = c.fd, .events = POLLIN};
        int timeout = nq_poll_timeout(ngtcp2_conn_get_expiry2(c.conn), nq_now_ns());
        int n = poll(&pfd, 1, timeout);
        if (nq_stop) {
            goto out;
        }
        if (n < 0 && errno != EINTR) {
            fprintf(stderr, "poll: %s\n", strerror(errno));
            goto out;
        }

        if (n > 0 && client_read(&c) != 0) {
            client_close(&c);
            goto out;
        }
        if (c.done) {
            /* Single exchange done: close the connection (application close). */
            ngtcp2_ccerr_set_application_error(&c.last_error, 0, NULL, 0);
            client_close(&c);
            rc = c.ok ? 0 : 1;
            goto out;
        }

        int rv = ngtcp2_conn_handle_expiry(c.conn, nq_now_ns());
        if (rv != 0) {
            fprintf(stderr, "ngtcp2_conn_handle_expiry: %s\n", ngtcp2_strerror(rv));
            goto out;
        }
    }

out:
    if (c.conn) {
        ngtcp2_conn_del(c.conn);
    }
    SSL_free(c.ssl);
    SSL_CTX_free(c.ssl_ctx);
    if (c.fd != -1) {
        close(c.fd);
    }
    return rc;
}
