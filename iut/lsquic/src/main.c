/*
 * Standalone lsquic client/server for the nesquic perf benchmark.
 * Wire protocol: docs/PROTOCOL.md. CLI: docs/CLI.md.
 */
#include "common.h"

int main(int argc, char **argv) {
    struct nq_args args;
    int rc = nq_parse_args(argc, argv, &args);
    if (rc != 0) {
        return rc;
    }

    if (lsquic_global_init(args.mode == NQ_CLIENT ? LSQUIC_GLOBAL_CLIENT
                                                  : LSQUIC_GLOBAL_SERVER) != 0) {
        fprintf(stderr, "lsquic_global_init failed\n");
        return 1;
    }

    nq_install_signal_handlers();
    rc = args.mode == NQ_CLIENT ? nq_run_client(&args) : nq_run_server(&args);

    lsquic_global_cleanup();
    return rc;
}
