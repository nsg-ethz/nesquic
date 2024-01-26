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

int create_socket(char *host, int port) {
	int sockfd;
	struct sockaddr_in dest_addr;

	sockfd = socket(AF_INET, SOCK_STREAM, 0);

	memset(&(dest_addr), '\0', sizeof(dest_addr));
	dest_addr.sin_family=AF_INET;
	dest_addr.sin_port=htons(port);
	dest_addr.sin_addr.s_addr = inet_addr(host);

	if ( connect(sockfd, (struct sockaddr *) &dest_addr, sizeof(struct sockaddr_in)) == -1 ) {
  		perror("Connect: ");
 		exit(-1);
	}

	return sockfd;
}

void client() {
    SSL_CTX *ctx = NULL;
	SSL *ssl = NULL;
	int socket = 0;
	int res = 0;

	if ( (ctx = SSL_CTX_new(SSLv23_client_method())) == NULL) {
   		printf("Unable to create a new SSL context structure.\n");
		exit(-1);
	}

	SSL_CTX_set_options(ctx, SSL_OP_NO_SSLv2);

	// enable kTLS
	res = SSL_CTX_set_options(ctx, SSL_OP_ENABLE_KTLS);
	if (res < 0) {
		printf("kTLS activation error: %i\n", res);
	}

	// Force gcm(aes) mode
	SSL_CTX_set_ciphersuites(ctx, "TLS_AES_128_GCM_SHA256");

	ssl = SSL_new(ctx);
	socket = create_socket("127.0.0.1", PORT);

	SSL_set_fd(ssl, socket);

	if ( SSL_connect(ssl) != 1 ) {
 		printf("Error: Could not build a SSL session\n");
		ERR_print_errors_fp(stderr);
		exit(-1);
	}

	// Start tests
	char buf[BUFSIZ];

	// strncpy(buf, "Hello openssl client", BUFSIZ);
	// printf("send(%s)\n", buf);
	// res = SSL_write(ssl, buf, sizeof(buf));
	// if (res < 0) {
	// 	printf("SSL Write error: %i\n", res);
	// }

	strncpy(buf, "Now using kTLS!\n", BUFSIZ);
	printf("CLIENT send(%s)\n", buf);
	res = send(socket, buf, sizeof(buf), 0);
	if (res < 0) {
		printf("CLIENT kTLS send error: %s\n", strerror(errno));
	}

	// bzero(buf, sizeof(buf));
	// res = recv(socket, buf, sizeof(buf), 0);
	// if (res < 0) {
	// 	printf("CLIENT kTLS recv error: %s\n", strerror(errno));
	// }
	// printf("CLIENT recv(%s)\n", buf);

	bzero(buf, sizeof(buf));
	res = SSL_read(ssl, buf, sizeof(buf));
	if (res < 0) {
		printf("SSL Read error: %i\n", res);
	}
	printf("recv(%s)\n", buf);

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

	client();

	return 0;
}
