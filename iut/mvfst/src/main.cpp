// Standalone mvfst client/server for the nesquic perf benchmark.
// Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
#include <folly/init/Init.h>

#include "common.h"

int main(int argc, char** argv) {
    nq_args args;
    int rc = nq_parse_args(argc, argv, &args);
    if (rc != 0) {
        return rc;
    }

    // Initialises folly (logging, singletons) without parsing our CLI.
    int fargc = 1;
    char** fargv = argv;
    folly::Init init(&fargc, &fargv, false);

    return args.mode == NQ_CLIENT ? nesquic::runClient(args) : nesquic::runServer(args);
}
