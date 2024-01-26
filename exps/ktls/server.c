#include <stdio.h>
#include <errno.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>
#include <netinet/in.h>
#include <arpa/inet.h>

#include <openssl/bio.h>
#include <openssl/ssl.h>
#include <openssl/err.h>
#include <openssl/pem.h>
#include <openssl/x509.h>
#include <openssl/x509_vfy.h>
#include <openssl/modes.h>
#include <openssl/aes.h>

#define PORT 12345
#define CRT_PEM "pem/cert.pem"
#define KEY_PEM "pem/key.pem"

int create_socket(int port) {
	int rc = -1, reuse = 1;
	struct sockaddr_in addr;

	int fd = socket(PF_INET, SOCK_STREAM, 0);
	if (fd < 0) {
		perror("Unable to create socket");
		goto end;
	}

	bzero(&addr, sizeof(addr));

	addr.sin_family = AF_INET;
	addr.sin_port = htons(port);
	addr.sin_addr.s_addr = htonl(INADDR_ANY);

	rc = setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(int));
	if (rc < 0) {
		perror("Unable to set SO_REUSEADDR");
		goto end;
	}

	rc = bind(fd, (const struct sockaddr*)&addr, sizeof(addr));
	if (rc < 0) {
		perror("Unable to bind");
		goto end;
	}

	rc = listen(fd, 10);
	if (rc < 0) {
		perror("Unable to listen");
		goto end;
	}
	rc = 0;
end:
	if (rc < 0 && fd >= 0) {
		close(fd);
		fd = -1;
	}
	return fd;
}

void server() {
    SSL_CTX *ctx = NULL;
	SSL *ssl = NULL;
	int socket = 0;
	int res = 0;

	if ( (ctx = SSL_CTX_new(SSLv23_server_method())) == NULL) {
   		printf("Unable to create a new SSL context structure.\n");
		exit(-1);
	}

	SSL_CTX_set_options(ctx, SSL_OP_NO_SSLv2);

	// Force gcm(aes) mode
	SSL_CTX_set_ciphersuites(ctx, "TLS_AES_128_GCM_SHA256");
	
	/* set the local certificate from CertFile */
	if ( SSL_CTX_use_certificate_file(ctx, CRT_PEM, SSL_FILETYPE_PEM) <= 0 ) {
	    ERR_print_errors_fp(stderr);
	    abort();
	}
	/* set the private key from KeyFile (may be the same as CertFile) */
	if ( SSL_CTX_use_PrivateKey_file(ctx, KEY_PEM, SSL_FILETYPE_PEM) <= 0 ) {
		ERR_print_errors_fp(stderr);
		abort();
	}
	/* verify private key */
	if ( !SSL_CTX_check_private_key(ctx) ) {
		fprintf(stderr, "Private key does not match the public certificate\n");
		abort();
	}

	socket = create_socket(PORT);

    while (1) {
		struct sockaddr_in addr;
		unsigned int len = sizeof(addr);

		/* accept connection as usual */
		int client = accept(socket, (struct sockaddr*) &addr, &len);

		/* accept connection as usual */
		ssl = SSL_new(ctx);
		/* set connection socket to SSL state */
		SSL_set_fd(ssl, client);

        // enable kTLS
        res = SSL_CTX_set_options(ctx, SSL_OP_ENABLE_KTLS);
        if (res < 0) {
            printf("kTLS activation error: %i\n", res);
        }

        res = SSL_accept(ssl);
        if (res < 0) {
            fprintf(stderr, "Failed to accept connection.\n");
            abort();
        }

		// /* service connection */
        char buf[BUFSIZ];

        // bzero(buf, sizeof(buf));
        // res = recv(client, buf, sizeof(buf), 0);
        // if (res < 0) {
        //     printf("SERVER kTLS recv error: %s\n", strerror(errno));
        // }
        // printf("SERVER recv(%s)\n", buf);

        bzero(buf, sizeof(buf));
        res = SSL_read(ssl, buf, sizeof(buf));
        if (res < 0) {
            printf("SSL Read error: %i\n", res);
        }
        printf("recv(%s)\n", buf);

        strncpy(buf, "Roger roger\n", BUFSIZ);
        printf("SERVER send(%s)\n", buf);
        res = send(client, buf, sizeof(buf), 0);
        if (res < 0) {
        	printf("SERVER kTLS send error: %s\n", strerror(errno));
        }

		SSL_free(ssl);
		/* close connection */
		close(client);
	}

	SSL_free(ssl);
	close(socket);
	SSL_CTX_free(ctx);
}

int main(int argv, char* argc[]) {
	SSL_library_init();
	OpenSSL_add_all_algorithms();
	ERR_load_crypto_strings();

	/* load all error messages */
	SSL_load_error_strings();

	server();

	return 0;
}
