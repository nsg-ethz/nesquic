#include "common.h"

#include <cstdio>
#include <cstring>

namespace nesquic {

const QUIC_API_TABLE* MsQuic = nullptr;
static HQUIC Registration = nullptr;

HQUIC registration() { return Registration; }

bool open_msquic() {
    QUIC_STATUS status = MsQuicOpen2(&MsQuic);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "MsQuicOpen2 failed: 0x%x\n", status);
        return false;
    }

    const QUIC_REGISTRATION_CONFIG reg_config = {
        "nesquic-msquic", QUIC_EXECUTION_PROFILE_LOW_LATENCY};
    status = MsQuic->RegistrationOpen(&reg_config, &Registration);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "RegistrationOpen failed: 0x%x\n", status);
        MsQuicClose(MsQuic);
        MsQuic = nullptr;
        return false;
    }
    return true;
}

void close_msquic() {
    if (Registration != nullptr) {
        MsQuic->RegistrationClose(Registration);
        Registration = nullptr;
    }
    if (MsQuic != nullptr) {
        MsQuicClose(MsQuic);
        MsQuic = nullptr;
    }
}

static QUIC_SETTINGS common_settings() {
    QUIC_SETTINGS settings;
    memset(&settings, 0, sizeof(settings));
    settings.IdleTimeoutMs = 10000;
    settings.IsSet.IdleTimeoutMs = TRUE;
    // The peer's stream-count limit defaults to 0, so the server must explicitly
    // allow the client to open bidirectional streams (see docs Streams.md).
    settings.PeerBidiStreamCount = 100;
    settings.IsSet.PeerBidiStreamCount = TRUE;
    return settings;
}

static HQUIC open_configuration(const QUIC_SETTINGS& settings) {
    QUIC_BUFFER alpn = {static_cast<uint32_t>(strlen(kAlpn)),
                        reinterpret_cast<uint8_t*>(const_cast<char*>(kAlpn))};
    HQUIC config = nullptr;
    QUIC_STATUS status = MsQuic->ConfigurationOpen(
        Registration, &alpn, 1, &settings, sizeof(settings), nullptr, &config);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ConfigurationOpen failed: 0x%x\n", status);
        return nullptr;
    }
    return config;
}

HQUIC make_client_configuration(const Args& args) {
    QUIC_SETTINGS settings = common_settings();
    HQUIC config = open_configuration(settings);
    if (config == nullptr) {
        return nullptr;
    }

    QUIC_CREDENTIAL_CONFIG cred;
    memset(&cred, 0, sizeof(cred));
    cred.Type = QUIC_CREDENTIAL_TYPE_NONE;
    cred.Flags = QUIC_CREDENTIAL_FLAG_CLIENT;
    // Trust the server's certificate via the supplied PEM (see PROTOCOL.md §2).
    // SET_CA_CERTIFICATE_FILE makes msquic honor CaCertificateFile and
    // USE_TLS_BUILTIN_CERTIFICATE_VALIDATION lets OpenSSL validate against it
    // (both are OpenSSL-only flags).
    if (!args.cert.empty()) {
        cred.Flags |= QUIC_CREDENTIAL_FLAG_SET_CA_CERTIFICATE_FILE;
        cred.Flags |= QUIC_CREDENTIAL_FLAG_USE_TLS_BUILTIN_CERTIFICATE_VALIDATION;
        cred.CaCertificateFile = args.cert.c_str();
    }

    QUIC_STATUS status = MsQuic->ConfigurationLoadCredential(config, &cred);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ConfigurationLoadCredential (client) failed: 0x%x\n", status);
        MsQuic->ConfigurationClose(config);
        return nullptr;
    }
    return config;
}

HQUIC make_server_configuration(const Args& args) {
    QUIC_SETTINGS settings = common_settings();
    HQUIC config = open_configuration(settings);
    if (config == nullptr) {
        return nullptr;
    }

    QUIC_CERTIFICATE_FILE cert_file;
    memset(&cert_file, 0, sizeof(cert_file));
    cert_file.CertificateFile = args.cert.c_str();
    cert_file.PrivateKeyFile = args.key.c_str();

    QUIC_CREDENTIAL_CONFIG cred;
    memset(&cred, 0, sizeof(cred));
    cred.Type = QUIC_CREDENTIAL_TYPE_CERTIFICATE_FILE;
    cred.Flags = QUIC_CREDENTIAL_FLAG_NONE;
    cred.CertificateFile = &cert_file;

    QUIC_STATUS status = MsQuic->ConfigurationLoadCredential(config, &cred);
    if (QUIC_FAILED(status)) {
        fprintf(stderr, "ConfigurationLoadCredential (server) failed: 0x%x\n", status);
        MsQuic->ConfigurationClose(config);
        return nullptr;
    }
    return config;
}

}  // namespace nesquic
