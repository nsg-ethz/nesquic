// Shared mvfst setup for the nesquic mvfst IUT.
#pragma once

#include <quic/state/TransportSettings.h>

#include "nesquic.h"

namespace nesquic {

// Transport settings shared by client and server. mvfst's defaults are
// conservative (65 KiB stream windows), which would cap throughput far below
// the other IUTs, so the windows are raised to comparable sizes.
inline quic::TransportSettings transportSettings() {
    quic::TransportSettings settings;
    settings.idleTimeout = std::chrono::milliseconds(10000);
    settings.advertisedInitialConnectionFlowControlWindow = 16 * 1024 * 1024;
    settings.advertisedInitialBidiLocalStreamFlowControlWindow = 8 * 1024 * 1024;
    settings.advertisedInitialBidiRemoteStreamFlowControlWindow = 8 * 1024 * 1024;
    settings.advertisedInitialUniStreamFlowControlWindow = 8 * 1024 * 1024;
    return settings;
}

int runClient(const nq_args& args);
int runServer(const nq_args& args);

}  // namespace nesquic
