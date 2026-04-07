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


## `cargo test` gives ``warning: preserving the entire environment is not supported, `-E` is ignored``
`sudo-rs` does not support `-E` but is the default on recent ubuntu-releases. See (sudo-rs/issues/1299)[https://github.com/trifectatechfoundation/sudo-rs/issues/1299#issuecomment-3567268773]

Solution:

```shell
ubuntu@cleansing-tarsier:~/nesquic$ sudo update-alternatives --config sudo
There are 2 choices for the alternative sudo (providing /usr/bin/sudo).

  Selection    Path                     Priority   Status
------------------------------------------------------------
* 0            /usr/lib/cargo/bin/sudo   50        auto mode
  1            /usr/bin/sudo.ws          40        manual mode
  2            /usr/lib/cargo/bin/sudo   50        manual mode

Press <enter> to keep the current choice[*], or type selection number: 1
update-alternatives: using /usr/bin/sudo.ws to provide /usr/bin/sudo (sudo) in manual mode
```
