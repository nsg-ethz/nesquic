#include "common.h"

#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <sys/uio.h>

#include <openssl/err.h>
#include <openssl/rand.h>

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

int nq_random(uint8_t *buf, size_t len) {
    return RAND_bytes(buf, len) == 1 ? 0 : -1;
}

void nq_rand_cb(uint8_t *dest, size_t destlen, const ngtcp2_rand_ctx *rand_ctx) {
    (void)rand_ctx;
    if (nq_random(dest, destlen) != 0) {
        abort();
    }
}

int nq_get_new_connection_id_cb(ngtcp2_conn *conn, ngtcp2_cid *cid,
                                ngtcp2_stateless_reset_token *token, size_t cidlen,
                                void *user_data) {
    (void)conn;
    (void)user_data;
    if (nq_random(cid->data, cidlen) != 0 ||
        nq_random(token->data, sizeof(token->data)) != 0) {
        return NGTCP2_ERR_CALLBACK_FAILURE;
    }
    cid->datalen = cidlen;
    return 0;
}

int nq_send_packet(int fd, const ngtcp2_path *path, const uint8_t *data, size_t len) {
    struct iovec iov = {.iov_base = (void *)data, .iov_len = len};
    struct msghdr msg;
    ssize_t n;

    memset(&msg, 0, sizeof(msg));
    msg.msg_name = path->remote.addr;
    msg.msg_namelen = path->remote.addrlen;
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;

    do {
        n = sendmsg(fd, &msg, 0);
    } while (n == -1 && errno == EINTR);

    if (n == -1) {
        /* A full socket buffer is just a lost packet to QUIC. */
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            return 0;
        }
        fprintf(stderr, "sendmsg: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

static SSL_CTX *new_ssl_ctx(void) {
    SSL_CTX *ctx = SSL_CTX_new(TLS_method());
    if (!ctx) {
        fprintf(stderr, "SSL_CTX_new: %s\n", ERR_error_string(ERR_get_error(), NULL));
        return NULL;
    }
    SSL_CTX_set_min_proto_version(ctx, TLS1_3_VERSION);
    SSL_CTX_set_max_proto_version(ctx, TLS1_3_VERSION);
    return ctx;
}

SSL_CTX *nq_client_ssl_ctx(const char *ca_file) {
    SSL_CTX *ctx = new_ssl_ctx();
    if (!ctx) {
        return NULL;
    }
    if (ngtcp2_crypto_boringssl_configure_client_context(ctx) != 0) {
        fprintf(stderr, "ngtcp2_crypto_boringssl_configure_client_context failed\n");
        goto fail;
    }
    /* Trust only the supplied certificate (see docs/PROTOCOL.md). */
    if (SSL_CTX_load_verify_locations(ctx, ca_file, NULL) != 1) {
        fprintf(stderr, "cannot load CA %s: %s\n", ca_file,
                ERR_error_string(ERR_get_error(), NULL));
        goto fail;
    }
    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);
    return ctx;

fail:
    SSL_CTX_free(ctx);
    return NULL;
}

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

SSL_CTX *nq_server_ssl_ctx(const char *cert_file, const char *key_file) {
    SSL_CTX *ctx = new_ssl_ctx();
    if (!ctx) {
        return NULL;
    }
    if (ngtcp2_crypto_boringssl_configure_server_context(ctx) != 0) {
        fprintf(stderr, "ngtcp2_crypto_boringssl_configure_server_context failed\n");
        goto fail;
    }
    if (SSL_CTX_use_certificate_chain_file(ctx, cert_file) != 1 ||
        SSL_CTX_use_PrivateKey_file(ctx, key_file, SSL_FILETYPE_PEM) != 1 ||
        SSL_CTX_check_private_key(ctx) != 1) {
        fprintf(stderr, "cannot load certificate/key: %s\n",
                ERR_error_string(ERR_get_error(), NULL));
        goto fail;
    }
    SSL_CTX_set_alpn_select_cb(ctx, alpn_select_cb, NULL);
    return ctx;

fail:
    SSL_CTX_free(ctx);
    return NULL;
}

int nq_poll_timeout(ngtcp2_tstamp expiry, ngtcp2_tstamp now) {
    uint64_t ms;
    if (expiry == UINT64_MAX) {
        return -1;
    }
    if (expiry <= now) {
        return 0;
    }
    /* Round up so we never wake before the timer is due. */
    ms = (expiry - now + NGTCP2_MILLISECONDS - 1) / NGTCP2_MILLISECONDS;
    return ms > INT_MAX ? INT_MAX : (int)ms;
}
