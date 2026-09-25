#include <csignal>
#include <cstdio>
#include <map>
#include <memory>
#include <vector>

#include <fizz/backend/openssl/certificate/CertUtils.h>
#include <fizz/protocol/clock/SystemClock.h>
#include <fizz/server/DefaultCertManager.h>
#include <fizz/server/FizzServerContext.h>
#include <folly/FileUtil.h>
#include <folly/Synchronized.h>
#include <folly/io/Cursor.h>
#include <quic/api/QuicSocket.h>
#include <quic/fizz/handshake/QuicFizzFactory.h>
#include <quic/server/QuicServer.h>
#include <quic/server/QuicServerTransport.h>

#include "common.h"

namespace nesquic {

namespace {

// Response bytes are zeros served from this buffer. Chunks wrap it without
// copying; it never changes, so mvfst may keep referencing it until acked.
constexpr size_t kZeroChunk = 64 * 1024;
const uint8_t kZeros[kZeroChunk] = {};

// Serves one connection: reads each request to EOF, then streams the blob as
// flow control allows.
class Handler : public quic::QuicSocket::ConnectionSetupCallback,
                public quic::QuicSocket::ConnectionCallback,
                public quic::QuicSocket::ReadCallback,
                public quic::QuicSocket::WriteCallback {
  public:
    explicit Handler(folly::EventBase* evb) : evb_(evb) {}

    void setSocket(std::shared_ptr<quic::QuicSocket> sock) { sock_ = std::move(sock); }
    folly::EventBase* eventBase() const { return evb_; }

    void onNewBidirectionalStream(quic::StreamId id) noexcept override {
        streams_[id] = Stream{};
        sock_->setReadCallback(id, this);
    }

    void readAvailable(quic::StreamId id) noexcept override {
        auto res = sock_->read(id, 0);
        if (res.hasError()) {
            sock_->setReadCallback(id, nullptr);
            return;
        }
        Stream& st = streams_[id];
        if (res->first) {
            // Only the leading 8 bytes are significant (docs/PROTOCOL.md).
            folly::io::Cursor cursor(res->first.get());
            while (st.requestLen < NQ_REQUEST_LEN && !cursor.isAtEnd()) {
                st.request[st.requestLen++] = cursor.read<uint8_t>();
            }
        }
        if (!res->second) {
            return;
        }

        sock_->setReadCallback(id, nullptr);
        if (st.requestLen < NQ_REQUEST_LEN) {
            sock_->resetStream(id, quic::GenericApplicationErrorCode::UNKNOWN);
            streams_.erase(id);
            return;
        }
        st.remaining = nq_request_decode(st.request);
        if (st.remaining == 0) {
            sock_->writeChain(id, nullptr, true);
            streams_.erase(id);
            return;
        }
        sock_->notifyPendingWriteOnStream(id, this);
    }

    void onStreamWriteReady(quic::StreamId id, uint64_t maxToSend) noexcept override {
        auto it = streams_.find(id);
        if (it == streams_.end()) {
            return;
        }
        Stream& st = it->second;

        uint64_t len = std::min(st.remaining, std::max<uint64_t>(maxToSend, kZeroChunk));
        std::unique_ptr<folly::IOBuf> chain;
        for (uint64_t off = 0; off < len; off += kZeroChunk) {
            auto chunk = folly::IOBuf::wrapBuffer(kZeros, std::min<uint64_t>(kZeroChunk, len - off));
            if (chain) {
                chain->appendToChain(std::move(chunk));
            } else {
                chain = std::move(chunk);
            }
        }
        st.remaining -= len;
        bool eof = st.remaining == 0;

        if (sock_->writeChain(id, std::move(chain), eof).hasError()) {
            streams_.erase(it);
            return;
        }
        if (eof) {
            streams_.erase(it);
        } else {
            sock_->notifyPendingWriteOnStream(id, this);
        }
    }

    void onStreamWriteError(quic::StreamId id, quic::QuicError) noexcept override {
        streams_.erase(id);
    }

    void readError(quic::StreamId id, quic::QuicError) noexcept override { streams_.erase(id); }

    void onNewUnidirectionalStream(quic::StreamId) noexcept override {}
    void onStopSending(quic::StreamId, quic::ApplicationErrorCode) noexcept override {}
    void onConnectionSetupError(quic::QuicError) noexcept override {}
    // The client closing the connection is the normal end of a run.
    void onConnectionError(quic::QuicError) noexcept override { streams_.clear(); }
    void onConnectionEnd() noexcept override { streams_.clear(); }

  private:
    struct Stream {
        uint8_t request[NQ_REQUEST_LEN] = {};
        size_t requestLen = 0;
        uint64_t remaining = 0;
    };

    folly::EventBase* evb_;
    std::shared_ptr<quic::QuicSocket> sock_;
    std::map<quic::StreamId, Stream> streams_;
};

class TransportFactory : public quic::QuicServerTransportFactory {
  public:
    ~TransportFactory() override {
        handlers_.withWLock([](auto& handlers) {
            while (!handlers.empty()) {
                auto& handler = handlers.back();
                handler->eventBase()->runImmediatelyOrRunInEventBaseThreadAndWait(
                    [&] { handlers.pop_back(); });
            }
        });
    }

    quic::QuicServerTransport::Ptr make(
        folly::EventBase* evb, std::unique_ptr<quic::FollyAsyncUDPSocketAlias> sock,
        const folly::SocketAddress& peer, quic::QuicVersion,
        std::shared_ptr<const fizz::server::FizzServerContext> ctx) noexcept override {
        fprintf(stderr, "new connection from %s\n", peer.describe().c_str());
        auto handler = std::make_unique<Handler>(evb);
        auto transport = quic::QuicServerTransport::make(evb, std::move(sock), handler.get(),
                                                         handler.get(), ctx);
        handler->setSocket(transport);
        handlers_.withWLock([&](auto& handlers) { handlers.push_back(std::move(handler)); });
        return transport;
    }

  private:
    folly::Synchronized<std::vector<std::unique_ptr<Handler>>> handlers_;
};

std::shared_ptr<fizz::server::FizzServerContext> makeServerContext(const char* certFile,
                                                                   const char* keyFile) {
    std::string certData, keyData;
    if (!folly::readFile(certFile, certData) || !folly::readFile(keyFile, keyData)) {
        fprintf(stderr, "cannot read certificate/key\n");
        return nullptr;
    }
    std::unique_ptr<fizz::SelfCert> cert;
    fizz::Error err;
    if (fizz::openssl::CertUtils::makeSelfCert(cert, err, certData, keyData) !=
        fizz::Status::Success) {
        fprintf(stderr, "cannot load certificate/key\n");
        return nullptr;
    }

    auto certManager = std::make_unique<fizz::server::DefaultCertManager>();
    certManager->addCertAndSetDefault(std::move(cert));

    auto ctx = std::make_shared<fizz::server::FizzServerContext>();
    ctx->setFactory(std::make_shared<quic::QuicFizzFactory>());
    ctx->setCertManager(std::move(certManager));
    ctx->setOmitEarlyRecordLayer(true);
    ctx->setClock(std::make_shared<fizz::SystemClock>());
    ctx->setSupportedAlpns({NQ_ALPN});
    return ctx;
}

}  // namespace

int runServer(const nq_args& args) {
    char host[256];
    uint16_t port;
    if (nq_split_host_port(args.listen, host, sizeof(host), &port) != 0) {
        fprintf(stderr, "malformed listen address: %s\n", args.listen);
        return 1;
    }
    auto ctx = makeServerContext(args.cert, args.key);
    if (!ctx) {
        return 1;
    }

    // Block SIGINT/SIGTERM before spawning worker threads so that only
    // sigwait() below receives them.
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGINT);
    sigaddset(&mask, SIGTERM);
    pthread_sigmask(SIG_BLOCK, &mask, nullptr);

    auto server = quic::QuicServer::createQuicServer(transportSettings());
    server->setQuicServerTransportFactory(std::make_unique<TransportFactory>());
    server->setFizzContext(ctx);

    folly::SocketAddress addr(host, port, true);
    // A single worker thread, like the other single-threaded IUTs.
    server->start(addr, 1);
    server->waitUntilInitialized();

    printf("Listening on %s\n", args.listen);
    fflush(stdout);

    int sig = 0;
    sigwait(&mask, &sig);

    server->shutdown();
    return 0;
}

}  // namespace nesquic
