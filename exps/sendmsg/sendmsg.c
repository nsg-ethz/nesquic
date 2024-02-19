#include <arpa/inet.h>
#include <netinet/ip.h>
#include <netinet/ip6.h>
#include <netinet/tcp.h>
#include <netinet/udp.h>
#include <errno.h>
#include <netdb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/socket.h>
#include <unistd.h>

#define ADDR "127.0.0.1" // this is stackoverflow.com :)
#define PORT 8080
#define SA struct sockaddr
#define CM struct cmsghdr

// definitions from https://github.com/torvalds/linux/blob/master/tools/testing/selftests/net/udpgso.c
#ifndef ETH_MAX_MTU
#define ETH_MAX_MTU	0xFFFFU
#endif

#ifndef UDP_MAX_SEGMENTS
#define UDP_MAX_SEGMENTS	(1 << 6UL)
#endif

#define CONST_MTU_TEST	1500

#define CONST_HDRLEN_V4		(sizeof(struct iphdr) + sizeof(struct udphdr))
#define CONST_HDRLEN_V6		(sizeof(struct ip6_hdr) + sizeof(struct udphdr))

#define CONST_MSS_V4		(CONST_MTU_TEST - CONST_HDRLEN_V4)
#define CONST_MSS_V6		(CONST_MTU_TEST - CONST_HDRLEN_V6)

#define CONST_MAX_SEGS_V4	(ETH_MAX_MTU / CONST_MSS_V4)
#define CONST_MAX_SEGS_V6	(ETH_MAX_MTU / CONST_MSS_V6)


ssize_t send_max_msg_tcp() {
    int sockfd, connfd;
    struct sockaddr_in servaddr, cli;
 
    // socket create and verification
    sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd == -1) {
        printf("socket creation failed...\n");
        exit(-1);
    }
    
    bzero(&servaddr, sizeof(servaddr));

    int max_seg_size = 9 * 1024;
    if (setsockopt(sockfd, IPPROTO_TCP, TCP_MAXSEG, &max_seg_size, sizeof(max_seg_size)) != 0) {
        printf("failed to set TCP max segment size...\n");
        exit(-1);
    }

    int send_buf_size = 32 * 1024 * 1024;
    if (setsockopt(sockfd, SOL_SOCKET, SO_SNDBUF, &send_buf_size, sizeof(send_buf_size)) != 0) {
        printf("failed to set socket send buffer size...\n");
        exit(-1);
    }
 
    // assign IP, PORT
    servaddr.sin_family = AF_INET;
    servaddr.sin_addr.s_addr = inet_addr(ADDR);
    servaddr.sin_port = htons(PORT);
 
    // connect the client socket to server socket
    if (connect(sockfd, (SA*)&servaddr, sizeof(servaddr)) != 0) {
        printf("connection with the server failed...\n");
        exit(-1);
    }

    // using write
    char buf[1024*1024*7];
    bzero(buf, sizeof(buf));
    ssize_t bytes_written_max = 0;
    for (int i = 0; i < 10; i++) {
        ssize_t bytes_written = write(sockfd, buf, sizeof(buf));
        if (bytes_written == -1) {
            printf("error writing: %s\n", strerror(errno));
            exit(-1);
        }
        // printf("wrote %ld bytes\n", bytes_written);
        if (bytes_written > bytes_written_max) bytes_written_max = bytes_written;
    }

    close(sockfd);

    return bytes_written_max;
}

ssize_t send_max_msg_udp() {
    int sockfd, connfd;
    struct sockaddr_in servaddr, cli;
 
    // socket create and verification
    sockfd = socket(AF_INET, SOCK_DGRAM, 0);
    if (sockfd == -1) {
        printf("socket creation failed...\n");
        exit(-1);
    }

    // enable GSO
    ssize_t gso_size = CONST_MSS_V4; // length of payload without headers, must be < MTU
    setsockopt(sockfd, SOL_UDP, UDP_SEGMENT, &gso_size, sizeof(gso_size));
    
    bzero(&servaddr, sizeof(servaddr));
 
    // assign IP, PORT
    servaddr.sin_family = AF_INET;
    servaddr.sin_addr.s_addr = inet_addr(ADDR);
    servaddr.sin_port = htons(PORT);
 
    // connect the client socket to server socket
    if (connect(sockfd, (SA*)&servaddr, sizeof(servaddr)) != 0) {
        printf("connection with the server failed...\n");
        exit(-1);
    }

    // using write
    // char buf[MAX];
    // bzero(buf, sizeof(buf));

    // ssize_t bytes_written = write(sockfd, buf, 63*1024);
    // if (bytes_written == -1) {
    //     printf("error writing: %s\n", strerror(errno));
    //     exit(-1);
    // }

    // using sendmsg
    size_t k = 1; // I think this should go up to UDP_MAX_SEGMENTS
    struct iovec iov[k];
    char buf[gso_size * CONST_MAX_SEGS_V4];

    // this only scatters/gathers, cannot be used to send more data
    for (int i = 0; i < k; i++) {
        iov[i].iov_base = buf;
        iov[i].iov_len = sizeof(buf);
    }

    struct msghdr hdr;
    memset(&hdr, 0, sizeof(hdr));    
    
    hdr.msg_iov = iov;
    hdr.msg_iovlen = k;

    char ctrl[CMSG_SPACE(sizeof(uint16_t))];
    hdr.msg_control = ctrl;
    hdr.msg_controllen = sizeof(ctrl);

    // enable UDP GSO
    CM* cm = CMSG_FIRSTHDR(&hdr);
    cm->cmsg_level = SOL_UDP;
    cm->cmsg_type = UDP_SEGMENT;
    cm->cmsg_len = CMSG_LEN(sizeof(uint16_t));
    uint16_t n = gso_size;
    memcpy(CMSG_DATA(cm), &n, sizeof(n));

    size_t bytes_written = sendmsg(sockfd, &hdr, 0);
    if (bytes_written == -1) {
        printf("error sending msg: %s\n", strerror(errno));
        exit(-1);
    }
 
    // close the socket
    close(sockfd);

    return bytes_written;
}

int main(int argc, char *argv[]) {
    printf("this bin finds the max payload one syscall can send.\n");

    ssize_t tcp_b = send_max_msg_tcp();
    float tcp_kb = tcp_b / 1024.0;
    printf("wrote %.2f KB on a TCP socket.\n", tcp_kb);

    ssize_t udp_b = send_max_msg_udp();
    float udp_kb = udp_b / 1024.0;
    printf("wrote %.2f KB on a UDP socket.\n", udp_kb);
}