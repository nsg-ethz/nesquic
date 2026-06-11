// Shared msquic setup and CLI definitions for the nesquic msquic IUT.
#pragma once

#include <string>

#include "msquic.h"

namespace nesquic {

// ALPN required by the nesquic perf protocol (see docs/PROTOCOL.md §2).
constexpr const char* kAlpn = "perf";
constexpr uint16_t kDefaultPort = 4433;

// The msquic API function table, initialised by open_msquic().
extern const QUIC_API_TABLE* MsQuic;

// The registration handle (execution context), valid after open_msquic().
HQUIC registration();

// Parsed command-line arguments shared by client and server.
struct Args {
    std::string cert;    // --cert: PEM certificate path
    std::string key;     // --key: PEM private key path (server only)
    std::string blob;    // --blob: requested size, e.g. "50Mbit" (client only)
    std::string url;     // client positional: server URL
    std::string listen;  // server positional: listen address:port
};

// Open the msquic library and registration. Returns true on success.
bool open_msquic();
void close_msquic();

// Build a Configuration handle with the perf ALPN, the common settings, and
// the given credentials. Returns nullptr on failure.
HQUIC make_client_configuration(const Args& args);
HQUIC make_server_configuration(const Args& args);

int run_client(const Args& args);
int run_server(const Args& args);

}  // namespace nesquic
