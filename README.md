# Better QUIC implementations with <img width="150" alt="nesquic" src="https://github.com/user-attachments/assets/5f5ab452-e3a6-41f3-a209-5ba2308f6188" />

Nesquic is a benchf infrastructure for QUIC libraries. It leverages eBPF to monitor library-internal QUIC components, like for example cryptography, or I/O. This allows the user to compare different design choices, find bottlenecks and improve the performance of their QUIC library.

Nesquic provides multiple QUIC client and server implementations (see [libraries](docs/LIBRARIES.md) for status of libraries), as well as a set of testing regimes to evaluate them. It leverages [Mahimahi](https://github.com/ravinet/mahimahi) to emulate realistic network conditions during the test, and eBPF to collect library-internal metrics. Nesquic then generates a Grafana dashboard with the results.
