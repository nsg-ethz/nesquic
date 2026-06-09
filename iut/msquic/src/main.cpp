// Standalone msquic client/server for the nesquic perf benchmark.
// Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "common.h"

namespace {

using nesquic::Args;

void usage(const char* prog) {
    fprintf(stderr,
            "usage:\n"
            "  %s client [--lib msquic] [-j JOB] [-L LABEL] --cert PEM --blob SIZE [URL]\n"
            "  %s server [--lib msquic] [-j JOB] [-L LABEL] --cert PEM --key PEM [LISTEN]\n",
            prog, prog);
}

// Returns the next argument value for a flag, or nullptr if missing.
const char* value(int argc, char** argv, int& i) {
    if (i + 1 >= argc) {
        return nullptr;
    }
    return argv[++i];
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        usage(argv[0]);
        return 2;
    }

    const std::string mode = argv[1];
    if (mode != "client" && mode != "server") {
        usage(argv[0]);
        return 2;
    }

    Args args;
    std::vector<std::string> positionals;

    for (int i = 2; i < argc; ++i) {
        const std::string arg = argv[i];
        if (arg == "-c" || arg == "--cert") {
            const char* v = value(argc, argv, i);
            if (!v) { usage(argv[0]); return 2; }
            args.cert = v;
        } else if (arg == "-k" || arg == "--key") {
            const char* v = value(argc, argv, i);
            if (!v) { usage(argv[0]); return 2; }
            args.key = v;
        } else if (arg == "-b" || arg == "--blob") {
            const char* v = value(argc, argv, i);
            if (!v) { usage(argv[0]); return 2; }
            args.blob = v;
        } else if (arg == "-l" || arg == "--lib" || arg == "-j" || arg == "--job" ||
                   arg == "-L" || arg == "--labels") {
            // Accepted for CLI compatibility with the nesquic harness; ignored
            // here since this build covers only the protocol (see docs/CLI.md).
            value(argc, argv, i);
        } else if (arg == "--unencrypted") {
            // Accepted but ignored, matching the other IUTs.
        } else if (!arg.empty() && arg[0] == '-') {
            fprintf(stderr, "unknown option: %s\n", arg.c_str());
            usage(argv[0]);
            return 2;
        } else {
            positionals.push_back(arg);
        }
    }

    if (mode == "client") {
        if (args.cert.empty() || args.blob.empty()) {
            fprintf(stderr, "client requires --cert and --blob\n");
            return 2;
        }
        args.url = positionals.empty() ? "https://127.0.0.1:4433" : positionals.front();
    } else {
        if (args.cert.empty() || args.key.empty()) {
            fprintf(stderr, "server requires --cert and --key\n");
            return 2;
        }
        args.listen = positionals.empty() ? "0.0.0.0:4433" : positionals.front();
    }

    if (!nesquic::open_msquic()) {
        return 1;
    }

    const int rc =
        (mode == "client") ? nesquic::run_client(args) : nesquic::run_server(args);

    nesquic::close_msquic();
    return rc;
}
