#include <cstdio>
#include <cstring>
#include <condition_variable>
#include <mutex>
#include <vector>

#include "common.h"
#include "nesquic.h"

namespace nesquic {

namespace {

struct ClientState {
    HQUIC connection = nullptr;   // owning connection, closed on SHUTDOWN_COMPLETE
    nq_load load;                 // touched only by the connection's callbacks
    bool done = false;            // signalled only from the terminal connection event
    std::mutex mutex;
    std::condition_variable cv;

    // Called exactly once, from the connection's SHUTDOWN_COMPLETE event (the
    // last callback msquic delivers) or if the connection never started.
    // Notifying under the lock lets the waiter safely destroy this object once
    // it wakes.
    void finish() {
        std::lock_guard<std::mutex> lock(mutex);
        done = true;
        cv.notify_one();
    }
};

// The stream context, freed on the stream's SHUTDOWN_COMPLETE.
struct Request {
    ClientState* state;
    QUIC_BUFFER send_buffer;      // kept alive for the send
    uint64_t begin_ns = 0;
    uint64_t received = 0;
};

void open_streams(ClientState* state);

QUIC_STATUS QUIC_API stream_callback(HQUIC stream, void* context, QUIC_STREAM_EVENT* event) {
    auto* request = static_cast<Request*>(context);
    auto* state = request->state;
    switch (event->Type) {
        case QUIC_STREAM_EVENT_RECEIVE:
            request->received += event->RECEIVE.TotalBufferLength;
            break;
        case QUIC_STREAM_EVENT_PEER_SEND_SHUTDOWN:
            // Server finished sending the blob.
            nq_load_end(&state->load, request->received, request->begin_ns);
            open_streams(state);
            break;
        case QUIC_STREAM_EVENT_SHUTDOWN_COMPLETE:
            MsQuic->StreamClose(stream);
            delete request;
            if (nq_load_done(&state->load)) {
                // Tear down the connection; finish() runs on its terminal event.
                MsQuic->ConnectionShutdown(state->connection,
                                           QUIC_CONNECTION_SHUTDOWN_FLAG_NONE, 0);
            }
            break;
        default:
            break;
    }
    return QUIC_STATUS_SUCCESS;
}

// Starts the pending requests; msquic queues streams beyond the server's
// stream limit until it is raised.
void open_streams(ClientState* state) {
    for (; state->load.pending; --state->load.pending) {
        auto* request = new Request{state, {NQ_REQUEST_LEN, state->load.request}};
        HQUIC stream = nullptr;
        QUIC_STATUS status = MsQuic->StreamOpen(state->connection, QUIC_STREAM_OPEN_FLAG_NONE,
                                                stream_callback, request, &stream);
        if (QUIC_FAILED(status)) {
            fprintf(stderr, "StreamOpen failed: 0x%x\n", status);
            delete request;
            break;
        }
        if (QUIC_FAILED(MsQuic->StreamStart(stream, QUIC_STREAM_START_FLAG_NONE))) {
            fprintf(stderr, "StreamStart failed\n");
            MsQuic->StreamClose(stream);
            delete request;
            break;
        }
        // Send the 8-byte request and close our send direction (FIN).
        request->begin_ns = nq_load_begin(&state->load);
        status = MsQuic->StreamSend(stream, &request->send_buffer, 1, QUIC_SEND_FLAG_FIN,
                                    nullptr);
        if (QUIC_FAILED(status)) {
            fprintf(stderr, "StreamSend failed: 0x%x\n", status);
            MsQuic->StreamShutdown(stream, QUIC_STREAM_SHUTDOWN_FLAG_ABORT, 0);
            break;
        }
    }
    if (state->load.pending) {
        state->load.failed = 1;
        MsQuic->ConnectionShutdown(state->connection, QUIC_CONNECTION_SHUTDOWN_FLAG_NONE, 0);
    }
}

QUIC_STATUS QUIC_API connection_callback(HQUIC connection, void* context,
                                         QUIC_CONNECTION_EVENT* event) {
    auto* state = static_cast<ClientState*>(context);
    switch (event->Type) {
        case QUIC_CONNECTION_EVENT_CONNECTED:
            open_streams(state);
            break;
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
    std::vector<ClientState> states(args.connections);
    for (auto& state : states) {
        if (nq_load_init(&state.load, args.blob.c_str(), args.streams, args.duration) != 0) {
            return 1;
        }
    }

    HQUIC config = make_client_configuration(args);
    if (config == nullptr) {
        return 1;
    }

    std::string host;
    uint16_t port = kDefaultPort;
    parse_url(args.url, host, port);

    for (auto& state : states) {
        QUIC_STATUS status = MsQuic->ConnectionOpen(registration(), connection_callback,
                                                    &state, &state.connection);
        if (QUIC_FAILED(status)) {
            fprintf(stderr, "ConnectionOpen failed: 0x%x\n", status);
            state.finish();
            continue;
        }

        status = MsQuic->ConnectionStart(state.connection, config, QUIC_ADDRESS_FAMILY_UNSPEC,
                                         host.c_str(), port);
        if (QUIC_FAILED(status)) {
            fprintf(stderr, "ConnectionStart failed: 0x%x\n", status);
            MsQuic->ConnectionClose(state.connection);
            state.finish();
        }
    }

    nq_load total = {};
    bool ok = true;
    for (auto& state : states) {
        std::unique_lock<std::mutex> lock(state.mutex);
        state.cv.wait(lock, [&] { return state.done; });
        ok = ok && nq_load_ok(&state.load);
        nq_load_merge(&total, &state.load);
    }
    nq_load_report(&total);

    MsQuic->ConfigurationClose(config);
    return ok ? 0 : 1;
}

}  // namespace nesquic
