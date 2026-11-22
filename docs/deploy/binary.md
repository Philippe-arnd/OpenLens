# Deploy the binary with systemd

For bare metal, a VM, an LXC container or a Raspberry Pi. The general guide is [README.md](README.md): requirements, release channels, updates, backups and settings.

The binaries are built for Linux `x86_64` and `aarch64` with glibc 2.35 or newer (Debian 12, Ubuntu 22.04 and later). The dashboard and the SDK are inside the binary: there is nothing else to install.

## Install

```sh
V=v0.2.0; A=$(uname -m)            # x86_64 or aarch64
curl -fsSLO https://github.com/Philippe-arnd/OpenLens/releases/download/$V/openlens-$V-$A-linux.tar.gz
tar xzf openlens-$V-$A-linux.tar.gz
sudo install -m 755 openlens-$V-$A-linux/openlens openlens-$V-$A-linux/openlens-cli /usr/local/bin/
sudo useradd --system --home /var/lib/openlens --shell /usr/sbin/nologin openlens
sudo install -d -o openlens -g openlens -m 750 /var/lib/openlens
```

`/etc/systemd/system/openlens.service`:

```ini
[Unit]
Description=OpenLens product analytics
After=network-online.target
Wants=network-online.target

[Service]
User=openlens
Group=openlens
Environment=OPENLENS_DATA_DIR=/var/lib/openlens
Environment=OPENLENS_ADDR=127.0.0.1:8686
Environment=OPENLENS_PUBLIC_URL=https://analytics.example.com
# EnvironmentFile=/etc/openlens.env   # optional settings and secrets
ExecStart=/usr/local/bin/openlens
Restart=on-failure
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=/var/lib/openlens

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload && sudo systemctl enable --now openlens
journalctl -u openlens | grep -i token   # the admin token, on first start
```

Then put a reverse proxy with HTTPS in front: recipes in [docker.md](docker.md#https-and-reverse-proxy) (point them at `127.0.0.1:8686`).

## Updates

Install the new binaries the same way, then `sudo systemctl restart openlens`. On start, OpenLens backs up its data when the version changed, and refuses data from a newer release. An `openlens update` command with a systemd timer, checksum verification and automatic rollback is tracked in [#238](https://github.com/Philippe-arnd/OpenLens/issues/238).

## Backups

A systemd timer or a cron line as the `openlens` user:

```sh
OPENLENS_DATA_DIR=/var/lib/openlens openlens backup --into /var/lib/openlens/backups/daily --keep 7
```

Then copy that directory off the machine.
