#include <cstdio>
#include <cstring>
#include <condition_variable>
#include <mutex>

#include "common.h"
#include "protocol.h"

namespace nesquic {

namespace {

struct ClientState {
    HQUIC connection = nullptr;   // owning connection, closed on SHUTDOWN_COMPLETE
    uint64_t requested = 0;       // bytes expected in the response
    uint64_t received = 0;        // bytes received so far
    uint8_t request[8];           // serialised request header (kept alive for send)
    QUIC_BUFFER send_buffer;
    bool ok = false;              // set as events progress; reported at the end
    bool done = false;            // signalled only from the terminal connection event
    std::mutex mutex;
    std::condition_variable cv;

    // Called exactly once, from the connection's SHUTDOWN_COMPLETE event (the
    // last callback msquic delivers). Notifying under the lock lets the waiter
    // safely destroy this object once it wakes.
    void finish() {
        std::lock_guard<std::mutex> lock(mutex);
        done = true;
        cv.notify_one();
    }
};

QUIC_STATUS QUIC_API stream_callback(HQUIC stream, void* context, QUIC_STREAM_EVENT* event) {
    auto* state = static_cast<ClientState*>(context);
    switch (event->Type) {
        case QUIC_STREAM_EVENT_RECEIVE:
            state->received += event->RECEIVE.TotalBufferLength;
            break;
        case QUIC_STREAM_EVENT_PEER_SEND_SHUTDOWN:
            // Server finished sending the blob; verify the length.
            state->ok = (state->received == state->requested);
            if (!state->ok) {
                fprintf(stderr,
                        "received blob size (%lluB) different from requested (%lluB)\n",
                        static_cast<unsigned long long>(state->received),
                        static_cast<unsigned long long>(state->requested));
            }
            break;
        case QUIC_STREAM_EVENT_SHUTDOWN_COMPLETE:
            MsQuic->StreamClose(stream);
            // Tear down the connection; finish() runs on its terminal event.
            MsQuic->ConnectionShutdown(state->connection,
                                       QUIC_CONNECTION_SHUTDOWN_FLAG_NONE, 0);
            break;
        default:
            break;
    }
    return QUIC_STATUS_SUCCESS;
}

QUIC_STATUS QUIC_API connection_callback(HQUIC connection, void* context,
                                         QUIC_CONNECTION_EVENT* event) {
    auto* state = static_cast<ClientState*>(context);
    switch (event->Type) {
        case QUIC_CONNECTION_EVENT_CONNECTED: {
            HQUIC stream = nullptr;
            QUIC_STATUS status = MsQuic->StreamOpen(
                connection, QUIC_STREAM_OPEN_FLAG_NONE, stream_callback, state, &stream);
            if (QUIC_FAILED(status)) {
                fprintf(stderr, "StreamOpen failed: 0x%x\n", status);
                MsQuic->ConnectionShutdown(connection, QUIC_CONNECTION_SHUTDOWN_FLAG_NONE, 0);
                break;
            }
            if (QUIC_FAILED(MsQuic->StreamStart(stream, QUIC_STREAM_START_FLAG_NONE))) {
                fprintf(stderr, "StreamStart failed\n");
                MsQuic->StreamClose(stream);
                MsQuic->ConnectionShutdown(connection, QUIC_CONNECTION_SHUTDOWN_FLAG_NONE, 0);
                break;
            }
            // Send the 8-byte request and close our send direction (FIN).
            state->send_buffer.Length = 8;
            state->send_buffer.Buffer = state->request;
            status = MsQuic->StreamSend(stream, &state->send_buffer, 1,
                                        QUIC_SEND_FLAG_FIN, nullptr);
            if (QUIC_FAILED(status)) {
                fprintf(stderr, "StreamSend failed: 0x%x\n", status);
                MsQuic->StreamShutdown(stream, QUIC_STREAM_SHUTDOWN_FLAG_ABORT, 0);
                MsQuic->ConnectionShutdown(connection, QUIC_CONNECTION_SHUTDOWN_FLAG_NONE, 0);
            }
            break;
        }
        case QUIC_CONNECTION_EVENT_SHUTDOWN_INITIATED_BY_TRANSPORT:
            fprintf(stderr, "connection shut down by transport: 0x%x\n",
                    event->SHUTDOWN_INITIATED_BY_TRANSPORT.Status);
            break;
        case QUIC_CONNECTION_EVENT_SHUTDOWN_COMPLETE:
            MsQuic->ConnectionClose(connection);
            state->finish();
            break;
        default:
            break;
    }
    return QUIC_STATUS_SUCCESS;
}

// Extract host and port from a URL like "https://host:port".
void parse_url(const std::string& url, std::string& host, uint16_t& port) {
    std::string rest = url;
    const size_t scheme = rest.find("://");
    if (scheme != std::string::npos) {
        rest = rest.substr(scheme + 3);
    }
    const size_t slash = rest.find('/');
    if (slash != std::string::npos) {
        rest = rest.substr(0, slash);
    }
    port = kDefaultPort;
    const size_t colon = rest.rfind(':');
    if (colon != std::string::npos) {
        host = rest.substr(0, colon);
        port = static_cast<uint16_t>(std::stoi(rest.substr(colon + 1)));
    } else {
        host = rest;
    }
}

}  // namespace

int run_client(const Args& args) {
    auto size = blob_bytes_from_string(args.blob);
    if (!size) {
        fprintf(stderr, "malformed blob size: %s\n", args.blob.c_str());
        return 1;
    }

    HQUIC config = make_client_configuration(args);
    if (config == nullptr) {
        return 1;
    }

    std::string host;
    uint16_t port = kDefaultPort;
    parse_url(args.url, host, port);

    ClientState state;
    state.requested = *size;
    request_to_bytes(*size, state.request);

    HQUIC connection = nullptr;
    QUIC_STATUS status = MsQuic->ConnectionOpen(registration(), connection_callback,
                                                &state, &connection);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ConnectionOpen failed: 0x%x\n", status);
        MsQuic->ConfigurationClose(config);
        return 1;
    }
    state.connection = connection;

    status = MsQuic->ConnectionStart(connection, config, QUIC_ADDRESS_FAMILY_UNSPEC,
                                     host.c_str(), port);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ConnectionStart failed: 0x%x\n", status);
        MsQuic->ConnectionClose(connection);
        MsQuic->ConfigurationClose(config);
        return 1;
    }

    std::unique_lock<std::mutex> lock(state.mutex);
    state.cv.wait(lock, [&] { return state.done; });
    const bool ok = state.ok;
    lock.unlock();

    MsQuic->ConfigurationClose(config);
    return ok ? 0 : 1;
}

}  // namespace nesquic
