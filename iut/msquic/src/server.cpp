#include <csignal>
#include <cstdio>
#include <cstring>
#include <vector>

#include "common.h"
#include "protocol.h"

namespace nesquic {

namespace {

// Per-stream state for the server: accumulates the 8-byte request and owns the
// zero-filled response buffer until the send completes.
struct StreamState {
    uint8_t request[8];
    size_t request_len = 0;
    std::vector<uint8_t> response;
    QUIC_BUFFER send_buffer;
};

QUIC_STATUS QUIC_API stream_callback(HQUIC stream, void* context, QUIC_STREAM_EVENT* event) {
    auto* state = static_cast<StreamState*>(context);
    switch (event->Type) {
        case QUIC_STREAM_EVENT_RECEIVE: {
            for (uint32_t i = 0; i < event->RECEIVE.BufferCount && state->request_len < 8; ++i) {
                const QUIC_BUFFER& buf = event->RECEIVE.Buffers[i];
                uint32_t take = buf.Length;
                if (state->request_len + take > 8) {
                    take = static_cast<uint32_t>(8 - state->request_len);
                }
                memcpy(state->request + state->request_len, buf.Buffer, take);
                state->request_len += take;
            }
            break;
        }
        case QUIC_STREAM_EVENT_PEER_SEND_SHUTDOWN: {
            // The client has sent the full request; serve the blob.
            if (state->request_len < 8) {
                MsQuic->StreamShutdown(stream, QUIC_STREAM_SHUTDOWN_FLAG_ABORT, 0);
                break;
            }
            const uint64_t size = request_from_bytes(state->request);
            state->response.assign(static_cast<size_t>(size), 0);
            state->send_buffer.Length = static_cast<uint32_t>(state->response.size());
            state->send_buffer.Buffer = state->response.data();
            QUIC_STATUS status = MsQuic->StreamSend(stream, &state->send_buffer, 1,
                                                    QUIC_SEND_FLAG_FIN, nullptr);
            if (QUIC_FAILED(status)) {
                fprintf(stderr, "StreamSend (response) failed: 0x%x\n", status);
                MsQuic->StreamShutdown(stream, QUIC_STREAM_SHUTDOWN_FLAG_ABORT, 0);
            }
            break;
        }
        case QUIC_STREAM_EVENT_SHUTDOWN_COMPLETE:
            MsQuic->StreamClose(stream);
            delete state;
            break;
        default:
            break;
    }
    return QUIC_STATUS_SUCCESS;
}

QUIC_STATUS QUIC_API connection_callback(HQUIC connection, void* /*context*/,
                                         QUIC_CONNECTION_EVENT* event) {
    switch (event->Type) {
        case QUIC_CONNECTION_EVENT_PEER_STREAM_STARTED: {
            auto* state = new StreamState();
            MsQuic->SetCallbackHandler(event->PEER_STREAM_STARTED.Stream,
                                       reinterpret_cast<void*>(stream_callback), state);
            break;
        }
        case QUIC_CONNECTION_EVENT_SHUTDOWN_COMPLETE:
            MsQuic->ConnectionClose(connection);
            break;
        default:
            break;
    }
    return QUIC_STATUS_SUCCESS;
}

QUIC_STATUS QUIC_API listener_callback(HQUIC /*listener*/, void* context,
                                       QUIC_LISTENER_EVENT* event) {
    HQUIC config = static_cast<HQUIC>(context);
    switch (event->Type) {
        case QUIC_LISTENER_EVENT_NEW_CONNECTION: {
            HQUIC connection = event->NEW_CONNECTION.Connection;
            MsQuic->SetCallbackHandler(connection,
                                       reinterpret_cast<void*>(connection_callback), nullptr);
            return MsQuic->ConnectionSetConfiguration(connection, config);
        }
        default:
            break;
    }
    return QUIC_STATUS_SUCCESS;
}

uint16_t parse_listen_port(const std::string& listen) {
    const size_t colon = listen.rfind(':');
    if (colon == std::string::npos) {
        return kDefaultPort;
    }
    return static_cast<uint16_t>(std::stoi(listen.substr(colon + 1)));
}

}  // namespace

int run_server(const Args& args) {
    HQUIC config = make_server_configuration(args);
    if (config == nullptr) {
        return 1;
    }

    // Block SIGINT/SIGTERM so we can wait for them to trigger shutdown.
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGINT);
    sigaddset(&mask, SIGTERM);
    pthread_sigmask(SIG_BLOCK, &mask, nullptr);

    HQUIC listener = nullptr;
    QUIC_STATUS status =
        MsQuic->ListenerOpen(registration(), listener_callback, config, &listener);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ListenerOpen failed: 0x%x\n", status);
        MsQuic->ConfigurationClose(config);
        return 1;
    }

    QUIC_ADDR addr;
    memset(&addr, 0, sizeof(addr));
    QuicAddrSetFamily(&addr, QUIC_ADDRESS_FAMILY_UNSPEC);
    QuicAddrSetPort(&addr, parse_listen_port(args.listen));

    QUIC_BUFFER alpn = {static_cast<uint32_t>(strlen(kAlpn)),
                        reinterpret_cast<uint8_t*>(const_cast<char*>(kAlpn))};
    status = MsQuic->ListenerStart(listener, &alpn, 1, &addr);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ListenerStart failed: 0x%x\n", status);
        MsQuic->ListenerClose(listener);
        MsQuic->ConfigurationClose(config);
        return 1;
    }

    printf("Listening on port %u\n", parse_listen_port(args.listen));
    fflush(stdout);

    int sig = 0;
    sigwait(&mask, &sig);

    MsQuic->ListenerClose(listener);
    MsQuic->ConfigurationClose(config);
    return 0;
}

}  // namespace nesquic
