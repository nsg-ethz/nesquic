# Troubleshooting `nesquic`
## Docker / prometheus
If the prometheus-container is trapped in a reboot-loop with the following error:
```
level=ERROR source=main.go:672 msg="Error loading config (--config.file=/etc/prometheus/prometheus.yml)" file=/etc/prometheus/prometheus.yml err="open /etc/prometheus/prometheus.yml: permission denied
```
do
```shell
chmod a+r -R docker
```
