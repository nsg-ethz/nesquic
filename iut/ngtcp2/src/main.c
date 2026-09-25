/*
 * Standalone ngtcp2 client/server for the nesquic perf benchmark.
 * Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
 */
#include "common.h"

int main(int argc, char **argv) {
    struct nq_args args;
    int rc = nq_parse_args(argc, argv, &args);
    if (rc != 0) {
        return rc;
    }

    nq_install_signal_handlers();
    return args.mode == NQ_CLIENT ? nq_run_client(&args) : nq_run_server(&args);
}
