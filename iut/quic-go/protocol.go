package main

import (
	"encoding/binary"
	"fmt"
	"net"
	"net/url"
	"strconv"
)

// blobBytes parses a blob-size string "<number>[G|M|K]bit" into a byte count
// (bits / 8, see docs/CLI.md).
func blobBytes(value string) (uint64, error) {
	if len(value) < 4 || value[len(value)-3:] != "bit" {
		return 0, fmt.Errorf("malformed blob size: %s", value)
	}
	num := value[:len(value)-3]
	mult := uint64(1)
	switch num[len(num)-1] {
	case 'G':
		mult = 1_000_000_000
	case 'M':
		mult = 1_000_000
	case 'K':
		mult = 1_000
	}
	if mult != 1 {
		num = num[:len(num)-1]
	}
	for _, c := range []byte(num) {
		if c < '0' || c > '9' {
			return 0, fmt.Errorf("malformed blob size: %s", value)
		}
	}
	n, err := strconv.ParseUint(num, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("malformed blob size: %s", value)
	}
	return n * mult / 8, nil
}

// encodeRequest serializes a byte count as the fixed 8-byte big-endian request.
func encodeRequest(size uint64) []byte {
	return binary.BigEndian.AppendUint64(nil, size)
}

// decodeRequest parses the 8-byte big-endian request.
func decodeRequest(b []byte) uint64 {
	return binary.BigEndian.Uint64(b[:requestLen])
}

// splitURL returns the host and "host:port" of a client URL; the port
// defaults to 4433.
func splitURL(raw string) (host, hostport string, err error) {
	u, err := url.Parse(raw)
	if err != nil {
		return "", "", err
	}
	host = u.Hostname()
	port := u.Port()
	if port == "" {
		port = strconv.Itoa(defaultPort)
	}
	return host, net.JoinHostPort(host, port), nil
}
