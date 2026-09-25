package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net"
	"os"
	"time"

	"github.com/quic-go/quic-go"
)

func quicConfig() *quic.Config {
	return &quic.Config{
		MaxIdleTimeout:        10 * time.Second,
		MaxIncomingStreams:    100,
		MaxIncomingUniStreams: -1,
	}
}

func runClient(ctx context.Context, a *args) error {
	size, err := blobBytes(a.blob)
	if err != nil {
		return err
	}
	host, hostport, err := splitURL(a.url)
	if err != nil {
		return err
	}
	remote, err := net.ResolveUDPAddr("udp", hostport)
	if err != nil {
		return err
	}

	// Trust only the supplied certificate (see docs/PROTOCOL.md).
	pem, err := os.ReadFile(a.cert)
	if err != nil {
		return err
	}
	roots := x509.NewCertPool()
	if !roots.AppendCertsFromPEM(pem) {
		return fmt.Errorf("no certificate in %s", a.cert)
	}
	tlsConf := &tls.Config{
		RootCAs:    roots,
		ServerName: host,
		NextProtos: []string{alpn},
		MinVersion: tls.VersionTLS13,
	}

	local := &net.UDPAddr{IP: net.IPv4zero}
	if remote.IP.To4() == nil {
		local.IP = net.IPv6unspecified
	}
	pc, err := listenLibc(local)
	if err != nil {
		return err
	}
	tr := &quic.Transport{Conn: pc}
	defer tr.Close()

	conn, err := tr.Dial(ctx, remote, tlsConf, quicConfig())
	if err != nil {
		return err
	}
	// Single exchange: close the connection (application close) when done.
	defer conn.CloseWithError(0, "")

	stream, err := conn.OpenStreamSync(ctx)
	if err != nil {
		return err
	}
	if _, err := stream.Write(encodeRequest(size)); err != nil {
		return err
	}
	// Finish the send side (FIN).
	if err := stream.Close(); err != nil {
		return err
	}

	received, err := io.Copy(io.Discard, stream)
	if err != nil {
		return err
	}
	if uint64(received) != size {
		return fmt.Errorf("received blob size (%dB) different from requested (%dB)", received, size)
	}
	return nil
}
