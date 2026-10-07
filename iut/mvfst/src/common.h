// Shared mvfst setup for the nesquic mvfst IUT.
#pragma once

#include <folly/io/SocketOptionMap.h>
#include <quic/state/TransportSettings.h>

#include "nesquic.h"

namespace nesquic {

// Transport settings shared by client and server (see docs/PROTOCOL.md).
// mvfst's defaults are conservative: 65 KiB stream windows, a 2000 packet
// congestion window cap, 5 unbatched packets per write. The I/O values are
// those of mvfst's own tperf tool.
inline quic::TransportSettings transportSettings() {
    quic::TransportSettings settings;
    settings.idleTimeout = std::chrono::milliseconds(10000);
    settings.advertisedInitialConnectionFlowControlWindow = NQ_CONNECTION_WINDOW;
    settings.advertisedInitialBidiLocalStreamFlowControlWindow = NQ_STREAM_WINDOW;
    settings.advertisedInitialBidiRemoteStreamFlowControlWindow = NQ_STREAM_WINDOW;
    settings.advertisedInitialUniStreamFlowControlWindow = NQ_STREAM_WINDOW;
    settings.advertisedInitialMaxStreamsBidi = NQ_MAX_STREAMS;
    settings.maxCwndInMss = quic::kLargeMaxCwndInMss;
    settings.writeConnectionDataPacketsLimit = 44;
    settings.batchingMode = quic::QuicBatchingMode::BATCHING_MODE_GSO;
    settings.maxBatchSize = 44;
    settings.shouldUseRecvmmsgForBatchRecv = true;
    settings.maxRecvBatchSize = 64;
    settings.numGROBuffers_ = 64;
    return settings;
}

// SO_RCVBUF/SO_SNDBUF shared with the other IUTs.
inline folly::SocketOptionMap socketOptions() {
    return {{{SOL_SOCKET, SO_RCVBUF}, NQ_SOCKET_BUFFER}, {{SOL_SOCKET, SO_SNDBUF}, NQ_SOCKET_BUFFER}};
}

int runClient(const nq_args& args);
int runServer(const nq_args& args);

}  // namespace nesquic
