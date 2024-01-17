# QUIC benchmark suite

## Remarks

* if localhost cannot be resolved, make sure that `/etc/hosts` contains `::1 localhost`
* to verify traffic amount sent use `sudo tshark -r dump.pcap | awk '{ sum += $7 } END { print sum }'`
* to install `perf` from source, use [this tutorial](https://medium.com/@manas.marwah/building-perf-tool-fc838f084f71), then add [assign the required capabilities](https://www.kernel.org/doc/html/latest/admin-guide/perf-security.html). To make sure that `sudo perf` actually invokes the binary you have just compiled, make sure to copy it to `/usr/bin` using `cp perf /usr/bin` inside `tools/perf`.