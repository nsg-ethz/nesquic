// Nesquic perf protocol helpers (see docs/PROTOCOL.md).
#pragma once

#include <cstdint>
#include <cstring>

namespace nesquic {

// Parse the 8-byte big-endian request header into a byte count.
inline uint64_t request_from_bytes(const uint8_t in[8]) {
    uint64_t size = 0;
    for (int i = 0; i < 8; ++i) {
        size = (size << 8) | static_cast<uint64_t>(in[i]);
    }
    return size;
}

}  // namespace nesquic
