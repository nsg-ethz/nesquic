// Standalone mvfst client/server for the nesquic perf benchmark.
// Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
#include "common.h"

int main(int argc, char** argv) {
    nq_args args;
    int rc = nq_parse_args(argc, argv, &args);
    if (rc != 0) {
        return rc;
    }
    return args.mode == NQ_CLIENT ? nesquic::runClient(args) : nesquic::runServer(args);
}
