# QUIC benchmark suite

## Remarks

* if localhost cannot be resolved, make sure that `/etc/hosts` contains `::1 localhost`
* to verify traffic amount sent use `sudo tshark -r dump.pcap | awk '{ sum += $7 } END { print sum }'`