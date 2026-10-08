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

/* Send buffer for ngtcp2_conn_write_aggregate_pkt: one GSO batch. */
#define NQ_GSO_BUFLEN 65535

/*
 * Sends `len` bytes as datagrams of `gsolen` bytes (the last may be shorter)
 * with a single sendmsg (UDP_SEGMENT). Returns 0 on success.
 */
int nq_send_packets(int fd, const ngtcp2_path *path, const uint8_t *data, size_t len,
                    size_t gsolen);

/* Builds the TLS contexts: ALPN "perf", TLS 1.3 only. */
SSL_CTX *nq_client_ssl_ctx(const char *ca_file);
SSL_CTX *nq_server_ssl_ctx(const char *cert_file, const char *key_file);

/* Waits until `fd` is readable or `expiry` passes. Returns like poll(). */
int nq_poll(int fd, ngtcp2_tstamp expiry);

/* Runs one connection (see nq_run_connections). */
int nq_run_client(const struct nq_args *args, struct nq_load *load);
int nq_run_server(const struct nq_args *args);

#endif
