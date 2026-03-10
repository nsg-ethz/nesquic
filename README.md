# Better QUIC implementations with <img width="150" alt="nesquic" src="https://github.com/user-attachments/assets/5f5ab452-e3a6-41f3-a209-5ba2308f6188" />

Nesquic is a testing infrastructure for QUIC libraries. It leverages eBPF to monitor library-internal QUIC components, like for example cryptography, or I/O. This allows the user to compare different design choices, find bottlenecks and improve the performance of their QUIC library.

## Status

| Library          | Status                                  |
|------------------|-----------------------------------------|
| [Quinn](https://github.com/quinn-rs/quinn)        | ✅     |
| [Quiche](https://github.com/cloudflare/quiche)    | ✅     |
| [MsQuic](https://github.com/microsoft/msquic)     | WIP    |
