/* Shared ngtcp2 setup for the nesquic ngtcp2 IUT. */
#ifndef NESQUIC_NGTCP2_COMMON_H
#define NESQUIC_NGTCP2_COMMON_H

#include <ngtcp2/ngtcp2.h>
#include <ngtcp2/ngtcp2_crypto.h>
#include <ngtcp2/ngtcp2_crypto_boringssl.h>
#include <openssl/ssl.h>

#include "nesquic.h"

/* Length of the connection IDs we issue. */
#define NQ_SCID_LEN 8
/* Largest UDP payload we hand to ngtcp2 for a single packet. */
#define NQ_MAX_UDP_PAYLOAD 1452
/* Idle timeout, matching the msquic IUT. */
#define NQ_IDLE_TIMEOUT (10 * NGTCP2_SECONDS)

/* Set by SIGINT/SIGTERM; the event loops exit once it is set. */
extern volatile int nq_stop;

/* Installs SIGINT/SIGTERM handlers that set nq_stop and interrupt poll(). */
void nq_install_signal_handlers(void);

/* Fills buf with cryptographically secure random bytes. */
int nq_random(uint8_t *buf, size_t len);

/* ngtcp2 callbacks shared by client and server. */
void nq_rand_cb(uint8_t *dest, size_t destlen, const ngtcp2_rand_ctx *rand_ctx);
int nq_get_new_connection_id_cb(ngtcp2_conn *conn, ngtcp2_cid *cid,
                                ngtcp2_stateless_reset_token *token, size_t cidlen,
                                void *user_data);

/* Sends one UDP datagram, retrying on EINTR. Returns 0 on success. */
int nq_send_packet(int fd, const ngtcp2_path *path, const uint8_t *data, size_t len);

/* Builds the TLS contexts: ALPN "perf", TLS 1.3 only. */
SSL_CTX *nq_client_ssl_ctx(const char *ca_file);
SSL_CTX *nq_server_ssl_ctx(const char *cert_file, const char *key_file);

/* Milliseconds until `expiry`, clamped to [0, INT_MAX], for poll(). */
int nq_poll_timeout(ngtcp2_tstamp expiry, ngtcp2_tstamp now);

int nq_run_client(const struct nq_args *args);
int nq_run_server(const struct nq_args *args);

#endif
