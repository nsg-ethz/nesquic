#include <csignal>
#include <cstdio>
#include <memory>
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
    Client(folly::EventBase* evb, uint64_t requested) : evb_(evb), requested_(requested) {}

    void setTransport(std::shared_ptr<quic::QuicClientTransport> transport) {
        transport_ = std::move(transport);
    }

    bool ok() const { return ok_; }
    bool ready() const { return ready_; }

    void onTransportReady() noexcept override {
        ready_ = true;
        auto stream = transport_->createBidirectionalStream();
        if (stream.hasError()) {
            fail("createBidirectionalStream failed");
            return;
        }
        transport_->setReadCallback(*stream, this);

        uint8_t request[NQ_REQUEST_LEN];
        nq_request_encode(requested_, request);
        // Write the request and finish the send side (FIN).
        auto res = transport_->writeChain(*stream, folly::IOBuf::copyBuffer(request, sizeof(request)),
                                          true);
        if (res.hasError()) {
            fail("writeChain failed");
        }
    }

    void readAvailable(quic::StreamId id) noexcept override {
        auto res = transport_->read(id, 0);
        if (res.hasError()) {
            fail("read failed");
            return;
        }
        if (res->first) {
            received_ += res->first->computeChainDataLength();
        }
        if (res->second) {
            ok_ = received_ == requested_;
            if (!ok_) {
                fprintf(stderr, "received blob size (%lluB) different from requested (%lluB)\n",
                        static_cast<unsigned long long>(received_),
                        static_cast<unsigned long long>(requested_));
            }
            transport_->setReadCallback(id, nullptr);
            // Single exchange done: close the connection (application close).
            transport_->close(std::nullopt);
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
        if (!ok_) {
            fprintf(stderr, "connection error: %s\n", quic::toString(error).c_str());
        }
        evb_->terminateLoopSoon();
    }

    void onConnectionEnd() noexcept override { evb_->terminateLoopSoon(); }

  private:
    void fail(const std::string& why) {
        fprintf(stderr, "%s\n", why.c_str());
        transport_->closeNow(std::nullopt);
        evb_->terminateLoopSoon();
    }

    folly::EventBase* evb_;
    std::shared_ptr<quic::QuicClientTransport> transport_;
    uint64_t requested_;
    uint64_t received_{0};
    bool ok_{false};
    bool ready_{false};
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
    uint64_t requested;
    if (nq_blob_bytes(args.blob, &requested) != 0) {
        fprintf(stderr, "malformed blob size: %s\n", args.blob);
        return 1;
    }
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
    Client client(&evb, requested);

    auto fizzCtx = std::make_shared<fizz::client::FizzClientContext>();
    fizzCtx->setSupportedAlpns({NQ_ALPN});
    auto handshakeCtx = quic::FizzClientQuicHandshakeContext::Builder()
                            .setFizzClientContext(std::move(fizzCtx))
                            .setCertificateVerifier(std::move(verifier))
                            .build();

    auto transport = std::make_shared<quic::QuicClientTransport>(
        qEvb, std::make_unique<quic::FollyQuicAsyncUDPSocket>(qEvb), std::move(handshakeCtx));
    if (!nq_is_ip_literal(host)) {
        transport->setHostname(host);
    }
    transport->addNewPeerAddress(folly::SocketAddress(host, port, true));
    transport->setTransportSettings(transportSettings());
    client.setTransport(transport);

    // SIGINT/SIGTERM cancel the job (docs/CLI.md). Without a handler they
    // would be ignored when the client runs as PID 1 in its container.
    auto onSignal = [](int) { _exit(1); };
    std::signal(SIGINT, onSignal);
    std::signal(SIGTERM, onSignal);

    transport->start(&client, &client);
    evb.runAfterDelay(
        [&] {
            if (!client.ready()) {
                fprintf(stderr, "handshake timed out\n");
                evb.terminateLoopSoon();
            }
        },
        kHandshakeTimeoutMs);
    evb.loopForever();

    // Detach from the transport before the event base goes away.
    transport->closeNow(std::nullopt);
    return client.ok() ? 0 : 1;
}

}  // namespace nesquic
