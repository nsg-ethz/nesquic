// Standalone quic-go client/server for the nesquic perf benchmark.
// Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"
)

const (
	alpn          = "perf"
	defaultPort   = 4433
	defaultURL    = "https://127.0.0.1:4433"
	defaultListen = "0.0.0.0:4433"
	requestLen    = 8
)

type args struct {
	client bool
	cert   string // --cert: PEM certificate path
	key    string // --key: PEM private key path (server only)
	blob   string // --blob: requested size, e.g. "50Mbit" (client only)
	url    string // client positional: server URL
	listen string // server positional: listen address:port
}

func usage() {
	fmt.Fprintf(os.Stderr, "usage:\n"+
		"  %[1]s client [-j JOB] [-L LABEL] --cert PEM --blob SIZE [URL]\n"+
		"  %[1]s server [-j JOB] [-L LABEL] --cert PEM --key PEM [LISTEN]\n", os.Args[0])
}

// parseArgs parses the container CLI. `-j`/`-L` are read by libnesquic.so
// from /proc/self/cmdline, so they are accepted and ignored here.
func parseArgs(argv []string) (*args, error) {
	if len(argv) < 1 || (argv[0] != "client" && argv[0] != "server") {
		return nil, fmt.Errorf("expected client or server")
	}
	a := &args{client: argv[0] == "client"}
	var positional string

	for i := 1; i < len(argv); i++ {
		arg := argv[i]
		var target *string
		switch arg {
		case "-c", "--cert":
			target = &a.cert
		case "-k", "--key":
			target = &a.key
		case "-b", "--blob":
			target = &a.blob
		case "-j", "--job", "-L", "--labels", "-l", "--lib", "--quic-cpu":
			i++
			continue
		case "--unencrypted":
			// Accepted but ignored, matching the other IUTs.
			continue
		default:
			switch {
			case strings.HasPrefix(arg, "-j"), strings.HasPrefix(arg, "-L"),
				strings.HasPrefix(arg, "--job="), strings.HasPrefix(arg, "--labels="):
				continue
			case strings.HasPrefix(arg, "-"):
				return nil, fmt.Errorf("unknown option: %s", arg)
			}
			positional = arg
			continue
		}
		i++
		if i >= len(argv) {
			return nil, fmt.Errorf("missing value for %s", arg)
		}
		*target = argv[i]
	}

	if a.client {
		if a.cert == "" || a.blob == "" {
			return nil, fmt.Errorf("client requires --cert and --blob")
		}
		a.url = defaultURL
		if positional != "" {
			a.url = positional
		}
	} else {
		if a.cert == "" || a.key == "" {
			return nil, fmt.Errorf("server requires --cert and --key")
		}
		a.listen = defaultListen
		if positional != "" {
			a.listen = positional
		}
	}
	return a, nil
}

func main() {
	a, err := parseArgs(os.Args[1:])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		usage()
		exit(2)
	}

	// SIGINT/SIGTERM cancel the job; the server relies on this for shutdown.
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)

	if a.client {
		err = runClient(ctx, a)
	} else {
		err = runServer(ctx, a)
	}
	stop()
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		exit(1)
	}
	exit(0)
}
