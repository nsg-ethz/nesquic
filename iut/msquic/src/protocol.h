// Nesquic perf protocol helpers (see docs/PROTOCOL.md).
#pragma once

#include <cstdint>
#include <cstring>
#include <optional>
#include <string>

namespace nesquic {

// Parse a blob-size string of the form "<number>[G|M|K]bit" into a byte count.
// The number is interpreted as bits; the result is bits / 8 (see PROTOCOL.md §3).
inline std::optional<uint64_t> blob_bytes_from_string(const std::string& value) {
    if (value.size() < 4 || value.compare(value.size() - 3, 3, "bit") != 0) {
        return std::nullopt;
    }

    const char prefix = value[value.size() - 4];
    uint64_t mult = 1;
    size_t num_end;  // one past the last digit of the numeric portion

    if (prefix >= '0' && prefix <= '9') {
        num_end = value.size() - 3;  // drop "bit"
    } else {
        switch (prefix) {
            case 'G': mult = 1000ULL * 1000 * 1000; break;
            case 'M': mult = 1000ULL * 1000; break;
            case 'K': mult = 1000ULL; break;
            default: return std::nullopt;
        }
        num_end = value.size() - 4;  // drop prefix + "bit"
    }

    if (num_end == 0) {
        return std::nullopt;
    }

    uint64_t n = 0;
    for (size_t i = 0; i < num_end; ++i) {
        const char c = value[i];
        if (c < '0' || c > '9') {
            return std::nullopt;
        }
        n = n * 10 + static_cast<uint64_t>(c - '0');
    }

    return (n * mult) / 8;
}

// Serialize a byte count as the fixed 8-byte big-endian request header.
inline void request_to_bytes(uint64_t size, uint8_t out[8]) {
    for (int i = 0; i < 8; ++i) {
        out[7 - i] = static_cast<uint8_t>((size >> (8 * i)) & 0xFF);
    }
}

// Parse the 8-byte big-endian request header into a byte count.
inline uint64_t request_from_bytes(const uint8_t in[8]) {
    uint64_t size = 0;
    for (int i = 0; i < 8; ++i) {
        size = (size << 8) | static_cast<uint64_t>(in[i]);
    }
    return size;
}

}  // namespace nesquic
