## Libraries

| Library          | Status                                  | Notes |
|------------------|-----------------------------------------|-------|
| [Quinn](https://github.com/quinn-rs/quinn)        | ✅     |       |
| [Quiche](https://github.com/cloudflare/quiche)    | ✅     |       |
| [MsQuic](https://github.com/microsoft/msquic)     | ✅    |       |
| [Neqo](https://github.com/mozilla/neqo)           | WIP    | Does not perform server authentication |
| [noq](https://github.com/n0-computer/noq)         | ✅    |        |
| [ngtcp2](https://github.com/ngtcp2/ngtcp2)        | ✅    | C, BoringSSL backend |
| [LSQUIC](https://github.com/litespeedtech/lsquic) | ✅    | C, BoringSSL |
| [XQUIC](https://github.com/alibaba/xquic)         | ✅    | C, BoringSSL. Client stream receive window pinned to 6 MiB to stay below XQUIC's 8192-frame reassembly limit |
| [mvfst](https://github.com/facebook/mvfst)        | WIP    | C++, fizz/OpenSSL. Flow control windows raised from mvfst's 65 KiB default |

The C IUTs (ngtcp2, LSQUIC, XQUIC) share the protocol and CLI helpers in
`iut/c-common/nesquic.h`.
