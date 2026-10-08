#include <csignal>
#include <cstdio>
#include <memory>
#include <unordered_map>
#include <vector>
#include <unistd.h>

#include <fizz/backend/openssl/certificate/OpenSSLCertificateVerifier.h>
#include <fizz/client/FizzClientContext.h>
#include <folly/io/async/EventBase.h>
#include <openssl/x509_vfy.h>
#include <quic/api/QuicSocket.h>
#include <quic/client/QuicClientTransport.h>
#include <quic/common/events/FollyQuicEventBase.h>
#include <quic/common/udpsocket/FollyQuicAsyncUDPSocket.h>
#include <quic/fizz/client/handshake/FizzClientQuicHandshakeContext.h>

#include "common.h"

namespace nesquic {

namespace {

class Client : public quic::QuicSocket::ConnectionSetupCallback,
               public quic::QuicSocket::ConnectionCallback,
               public quic::QuicSocket::ReadCallback {
  public:
    // `running` counts the connections sharing `evb`; the last to finish stops it.
    Client(folly::EventBase* evb, size_t* running) : evb_(evb), running_(running) {}

    void setTransport(std::shared_ptr<quic::QuicClientTransport> transport) {
        transport_ = std::move(transport);
    }

    nq_load* load() { return &load_; }
    bool ready() const { return ready_; }

    void onTransportReady() noexcept override {
        ready_ = true;
        openStreams();
    }

    // The server raised the stream limit.
    void onBidirectionalStreamsAvailable(uint64_t) noexcept override { openStreams(); }

    void readAvailable(quic::StreamId id) noexcept override {
        auto res = transport_->read(id, 0);
        if (res.hasError()) {
            fail("read failed");
            return;
        }
        auto& request = requests_[id];
        if (res->first) {
            request.received += res->first->computeChainDataLength();
        }
        if (res->second) {
            nq_load_end(&load_, request.received, request.beginNs);
            requests_.erase(id);
            transport_->setReadCallback(id, nullptr);
            openStreams();
            if (nq_load_done(&load_)) {
                // All exchanges done: close the connection (application close).
                // mvfst drops the connection callbacks on a local close, so
                // onConnectionEnd never fires; leave the loop here instead.
                transport_->close(std::nullopt);
                finish();
            }
        }
    }

    void readError(quic::StreamId, quic::QuicError error) noexcept override {
        fail(quic::toString(error));
    }

    void onNewBidirectionalStream(quic::StreamId) noexcept override {}
    void onNewUnidirectionalStream(quic::StreamId) noexcept override {}
    void onStopSending(quic::StreamId, quic::ApplicationErrorCode) noexcept override {}

    void onConnectionSetupError(quic::QuicError error) noexcept override {
        onConnectionError(std::move(error));
    }

    void onConnectionError(quic::QuicError error) noexcept override {
        if (!nq_load_ok(&load_)) {
            fprintf(stderr, "connection error: %s\n", quic::toString(error).c_str());
        }
        finish();
    }

    void onConnectionEnd() noexcept override { finish(); }

  private:
    struct Request {
        uint64_t beginNs{0};
        uint64_t received{0};
    };

    // Starts the pending requests as far as the server's stream limit allows.
    void openStreams() {
        while (load_.pending && transport_->getNumOpenableBidirectionalStreams() > 0) {
            auto stream = transport_->createBidirectionalStream();
            if (stream.hasError()) {
                fail("createBidirectionalStream failed");
                return;
            }
            --load_.pending;
            transport_->setReadCallback(*stream, this);
            requests_[*stream].beginNs = nq_load_begin(&load_);
            // Write the request and finish the send side (FIN).
            auto res = transport_->writeChain(
                *stream, folly::IOBuf::copyBuffer(load_.request, sizeof(load_.request)), true);
            if (res.hasError()) {
                fail("writeChain failed");
                return;
            }
        }
    }

    void fail(const std::string& why) {
        fprintf(stderr, "%s\n", why.c_str());
        load_.failed = 1;
        transport_->closeNow(std::nullopt);
        finish();
    }

    void finish() {
        if (!finished_ && --*running_ == 0) {
            evb_->terminateLoopSoon();
        }
        finished_ = true;
    }

    folly::EventBase* evb_;
    size_t* running_;
    std::shared_ptr<quic::QuicClientTransport> transport_;
    nq_load load_{};
    std::unordered_map<quic::StreamId, Request> requests_;
    bool ready_{false};
    bool finished_{false};
};

// mvfst's idle timeout does not cover the handshake, so an unreachable server
// would keep the client retransmitting Initials forever.
constexpr uint32_t kHandshakeTimeoutMs = 10000;

// Trusts only the supplied certificate and checks it against the URL host
// (see docs/PROTOCOL.md). The host/IP check is set on the store's
// parameters, which every verification inherits.
std::shared_ptr<const fizz::CertificateVerifier> makeVerifier(const char* caFile,
                                                              const char* host) {
    folly::ssl::X509StoreUniquePtr store(X509_STORE_new());
    if (!store || X509_STORE_load_locations(store.get(), caFile, nullptr) != 1) {
        fprintf(stderr, "cannot load CA %s\n", caFile);
        return nullptr;
    }
    X509_VERIFY_PARAM* param = X509_STORE_get0_param(store.get());
    if (nq_is_ip_literal(host)) {
        X509_VERIFY_PARAM_set1_ip_asc(param, host);
    } else {
        X509_VERIFY_PARAM_set1_host(param, host, 0);
    }

    std::unique_ptr<fizz::openssl::OpenSSLCertificateVerifier> verifier;
    fizz::Error err;
    if (fizz::openssl::OpenSSLCertificateVerifier::create(verifier, err,
                                                          fizz::VerificationContext::Client,
                                                          std::move(store)) !=
        fizz::Status::Success) {
        fprintf(stderr, "cannot create certificate verifier\n");
        return nullptr;
    }
    return verifier;
}

}  // namespace

int runClient(const nq_args& args) {
    char host[256];
    uint16_t port;
    if (nq_split_host_port(args.url, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed url: %s\n", args.url);
        return 1;
    }

    auto verifier = makeVerifier(args.cert, host);
    if (!verifier) {
        return 1;
    }

    folly::EventBase evb;
    auto qEvb = std::make_shared<quic::FollyQuicEventBase>(&evb);
    size_t running = args.connections;
    std::vector<std::unique_ptr<Client>> clients;
    std::vector<std::shared_ptr<quic::QuicClientTransport>> transports;

    // SIGINT/SIGTERM cancel the job (docs/CLI.md). Without a handler they
    // would be ignored when the client runs as PID 1 in its container.
    auto onSignal = [](int) { _exit(1); };
    std::signal(SIGINT, onSignal);
    std::signal(SIGTERM, onSignal);

    for (unsigned i = 0; i < args.connections; ++i) {
        auto& client = clients.emplace_back(std::make_unique<Client>(&evb, &running));
        if (nq_load_init(client->load(), args.blob, args.streams, args.duration) != 0) {
            return 1;
        }

        auto fizzCtx = std::make_shared<fizz::client::FizzClientContext>();
        fizzCtx->setSupportedAlpns({NQ_ALPN});
        auto handshakeCtx = quic::FizzClientQuicHandshakeContext::Builder()
                                .setFizzClientContext(std::move(fizzCtx))
                                .setCertificateVerifier(verifier)
                                .build();

        auto& transport = transports.emplace_back(std::make_shared<quic::QuicClientTransport>(
            qEvb, std::make_unique<quic::FollyQuicAsyncUDPSocket>(qEvb), std::move(handshakeCtx)));
        if (!nq_is_ip_literal(host)) {
            transport->setHostname(host);
        }
        transport->addNewPeerAddress(folly::SocketAddress(host, port, true));
        auto settings = transportSettings();
        settings.connectUDP = true;
        transport->setTransportSettings(settings);
        transport->setSocketOptions(socketOptions());
        client->setTransport(transport);
        transport->start(client.get(), client.get());
    }

    evb.runAfterDelay(
        [&] {
            for (auto& client : clients) {
                if (!client->ready()) {
                    fprintf(stderr, "handshake timed out\n");
                    evb.terminateLoopSoon();
                    return;
                }
            }
        },
        kHandshakeTimeoutMs);
    evb.loopForever();

    nq_load total = {};
    bool ok = true;
    for (size_t i = 0; i < clients.size(); ++i) {
        // Detach from the transport before the event base goes away.
        transports[i]->closeNow(std::nullopt);
        ok = ok && nq_load_ok(clients[i]->load());
        nq_load_merge(&total, clients[i]->load());
    }
    nq_load_report(&total);
    return ok ? 0 : 1;
}

}  // namespace nesquic
