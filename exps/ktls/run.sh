#! /bin/sh

echo "Compile the client"
clang -lssl -lcrypto client.c -o client
clang -lssl -lcrypto server.c -o server

echo "Enable kTLS"
modprobe tls

echo "Start TLS client & server"
./server
./client

# echo "Start TLS server"
# ncat -lvnp 12345 --ssl