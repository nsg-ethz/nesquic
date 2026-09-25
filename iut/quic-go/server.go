package main

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"net"
	"os"

	"github.com/quic-go/quic-go"
)

// Response bytes are zeros served from this buffer.
var zeros = make([]byte, 64*1024)

func runServer(ctx context.Context, a *args) error {
	cert, err := tls.LoadX509KeyPair(a.cert, a.key)
	if err != nil {
		return err
	}
	tlsConf := &tls.Config{
		Certificates: []tls.Certificate{cert},
		NextProtos:   []string{alpn},
		MinVersion:   tls.VersionTLS13,
	}

	addr, err := net.ResolveUDPAddr("udp", a.listen)
	if err != nil {
		return err
	}
	pc, err := listenLibc(addr)
	if err != nil {
		return err
	}
	tr := &quic.Transport{Conn: pc}
	defer tr.Close()

	ln, err := tr.Listen(tlsConf, quicConfig())
	if err != nil {
		return err
	}
	defer ln.Close()

	fmt.Printf("Listening on %s\n", a.listen)

	for {
		conn, err := ln.Accept(ctx)
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return err
		}
		go handleConn(ctx, conn)
	}
}

func handleConn(ctx context.Context, conn *quic.Conn) {
	for {
		stream, err := conn.AcceptStream(ctx)
		if err != nil {
			// The client closing the connection is the normal end of a run.
			var appErr *quic.ApplicationError
			if !(errors.As(err, &appErr) && appErr.Remote) && ctx.Err() == nil {
				fmt.Fprintln(os.Stderr, "connection error:", err)
			}
			return
		}
		go handleStream(stream)
	}
}

func handleStream(stream *quic.Stream) {
	// Only the leading 8 bytes are significant (docs/PROTOCOL.md).
	req, err := io.ReadAll(io.LimitReader(stream, 64*1024))
	if err != nil || len(req) < requestLen {
		stream.CancelWrite(0)
		return
	}

	remaining := decodeRequest(req)
	for remaining > 0 {
		n := uint64(len(zeros))
		if remaining < n {
			n = remaining
		}
		if _, err := stream.Write(zeros[:n]); err != nil {
			return
		}
		remaining -= n
	}
	stream.Close()
}
