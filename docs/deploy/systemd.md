# Deploy the binary with systemd

For bare metal, a VM, an LXC container or a Raspberry Pi. The general guide is [README.md](README.md): requirements, release channels, backups and settings.

The binaries are built for Linux `x86_64` and `aarch64` with glibc 2.35 or newer (Debian 12, Ubuntu 22.04 and later). The dashboard and the SDK are inside the binary: there is nothing else to install. The files this guide uses are in [`deploy/systemd/`](../../deploy/systemd/):

| File | What it is |
| --- | --- |
| `openlens.service` | the server, as a dedicated user, sandboxed, with a memory cap |
| `openlens-update` | installs or updates to the newest release of a channel, verified, and rolls back if it is not healthy |
| `openlens-update@.service`, `openlens-update@.timer` | run `openlens-update` once a day for the channel in the unit name |
| `install.sh` | does the steps of [Install](#install) below in one go, and enables the daily updates |

## Install in one command

```sh
curl -fsSL https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/systemd/install.sh \
  | sudo OPENLENS_PUBLIC_URL=https://analytics.example.com sh -s -- 0.6
```

`install.sh` (POSIX `sh`) takes the channel (`latest`, or a minor such as `0.6`; without it, the minor of the newest stable release) and:

1. checks the machine: root, systemd, Linux `x86_64` or `aarch64` with glibc, and the tools of [Install](#install) (`curl`, `jq`, `gh` 2.49 or newer). It installs no package: it says what is missing;
2. finds the newest stable release of the channel and downloads the four files of the table above **from that release's tag**, so they match the binary. Releases before 0.6.0 are refused, since they are not signed;
3. creates the `openlens` user and `/etc/openlens/openlens.env` (root only) with `OPENLENS_PUBLIC_URL` when you set it, as above. An existing file is kept;
4. runs `openlens-update <channel>`, which downloads the release and checks `SHA256SUMS` and the build provenance before it installs anything ([Updates](#updates));
5. enables `openlens.service` and the daily `openlens-update@<channel>.timer` ([Hands-off updates](#hands-off-updates)), waits for `/healthz` and says where the first start's owner link is.

Running it again updates the systemd files and the release (with the rollback of `openlens-update`); the data, the env file and the user stay. It also moves the older install described below. The script itself comes from `main` over HTTPS: read it first if you prefer, it is short. The step-by-step install follows.

## Install

`openlens-update` needs `curl`, `jq` and GitHub's `gh` 2.49 or newer, which checks the build provenance. Distribution packages of `gh` are often older: install it from [GitHub's own package repository](https://github.com/cli/cli/blob/trunk/docs/install_linux.md). It needs no GitHub login.

```sh
sudo apt install curl jq          # and gh, see above
R=https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/systemd
sudo curl -fsSL -o /usr/local/sbin/openlens-update $R/openlens-update
sudo chmod 755 /usr/local/sbin/openlens-update
for f in openlens.service openlens-update@.service openlens-update@.timer; do
  sudo curl -fsSL -o /etc/systemd/system/$f $R/$f
done
sudo useradd --system --home-dir /var/lib/openlens --shell /usr/sbin/nologin openlens
sudo openlens-update 0.6           # or `latest`: downloads, verifies, installs in /opt/openlens
```

`openlens-update` installs signed releases only, from 0.6.0 on. Until 0.6.0 is out, install 0.5 with the steps of [Update by hand](#update-by-hand-without-the-script) (without the `gh attestation verify` and `previous` lines, and with `sudo ln -sfn /opt/openlens/v0.5.0/openlens /usr/local/bin/openlens` and the same for `openlens-cli`); the timer then moves to 0.6.0 when it ships.

Settings are optional (the list is in [README.md](README.md#optional-settings-d-016)). The public URL is the one most installs set; keep secrets in this file too, readable by root only:

```sh
sudo install -d -m 700 /etc/openlens
echo 'OPENLENS_PUBLIC_URL=https://analytics.example.com' | sudo tee /etc/openlens/openlens.env
sudo chmod 600 /etc/openlens/openlens.env
sudo systemctl daemon-reload && sudo systemctl enable --now openlens
journalctl -u openlens | grep -i owner    # first start: a single-use owner link, valid 1 hour
```

OpenLens listens on `127.0.0.1:8686`. Put a reverse proxy with HTTPS in front: recipes in [docker.md](docker.md#https-and-reverse-proxy).

**Coming from the older install of this guide** (binaries in `/usr/local/bin`, user `openlens`, data in `/var/lib/openlens`): keep the user and the data, replace the unit with the one above, run `sudo openlens-update 0.6`, then `sudo systemctl daemon-reload && sudo systemctl restart openlens`. `/usr/local/bin/openlens` and `openlens-cli` become links to the release in use.

### What the unit sets

- **Its own user and data:** user `openlens`, data in `/var/lib/openlens` (`StateDirectory`, mode 750, files created 600). The binaries in `/opt/openlens` belong to root, so the server cannot change them.
- **Memory:** `MemoryMax=1G`, which covers about 4 M events in 90 days. OpenLens reads that limit and sizes its query memory to it, up to 768 MiB on any host (D-097), so a larger limit is not needed (the [requirements](README.md#requirements) have the measurements).
- **Sandbox:** no capabilities, no new privileges, the system read-only except the data directory, no home directories, private `/tmp` and devices, kernel settings, modules, logs and clock protected, IPv4, IPv6 and Unix sockets only, the `@system-service` system calls without the privileged ones, no writable and executable memory. `systemd-analyze security openlens` rates it 1.4 ("OK"). `/proc` stays readable, because OpenLens reads its memory limit there.
- **Long first start after an update:** a new version first copies the data to `backups/upgrade/`, before it listens, so the unit has no start timeout.

Keep `OPENLENS_ADDR` and `OPENLENS_DATA_DIR` in the unit (or in a drop-in from `systemctl edit`), not in the env file: `openlens-update` reads them from the unit to find the health check and the upgrade backup.

## Updates

```sh
sudo openlens-update 0.6
```

For the channel you give (`latest`, or a minor such as `0.6` for its patches only; never `edge` or pre-releases), it:

1. finds the newest release of the channel on GitHub. Same or older than the one installed: nothing to do;
2. downloads the archive for this machine and `SHA256SUMS`, checks the sum, then the build provenance with `gh attestation verify`, as in [Verify what you run](README.md#verify-what-you-run): signed through Sigstore by the release workflow of this repository, on GitHub's runners. Releases before 0.6 are not signed, so it refuses them;
3. unpacks it in `/opt/openlens/<tag>/`, points `/opt/openlens/current` to it (and `/opt/openlens/previous` to the release before), and restarts `openlens`. On start, OpenLens backs up its data when the version changed and refuses data from a newer release ([README.md](README.md#updates-without-anyone));
4. waits for `/healthz`, up to 15 minutes, since the upgrade backup comes first. A crash or a restart fails at once;
5. if it is not healthy: goes back to the previous release and restarts it. If that one refuses the data (the new release wrote a newer data format), it puts back the upgrade backup the new release made (the event, replay and rollup files and `meta.db`; keys, icons and other backups stay) and starts again. The failed release goes into `/opt/openlens/failed`, and later runs skip it until a newer one is out. The command exits with an error, so the failure shows in `systemctl --failed` and the journal.

It keeps the three newest releases in `/opt/openlens`.

### Hands-off updates

```sh
sudo systemctl enable --now openlens-update@0.6.timer      # patches of 0.6
# or: sudo systemctl enable --now openlens-update@latest.timer
systemctl list-timers 'openlens-update*'
journalctl -u 'openlens-update*'
```

The timer runs `openlens-update <channel>` once a day, between 04:00 and 05:00, and catches up after the machine was off. A minor channel brings fixes without behaviour changes. Moving to the next minor is a choice: read its **Changed** section in the [CHANGELOG](../../CHANGELOG.md), then disable the old timer and enable the new one. To be told about a failed update, add an `OnFailure=` unit to `openlens-update@.service` with `systemctl edit`, or watch `systemctl --failed`. The dashboard footer shows *Update available* either way.

### Update by hand, without the script

```sh
V=v0.6.0; A=$(uname -m)              # x86_64 or aarch64
F=openlens-$V-$A-linux.tar.gz
curl -fsSLO https://github.com/Philippe-arnd/OpenLens/releases/download/$V/$F
curl -fsSLO https://github.com/Philippe-arnd/OpenLens/releases/download/$V/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS
gh attestation verify $F -R Philippe-arnd/OpenLens     # needs a GitHub login this way
tar xzf $F
sudo install -d /opt/openlens/$V
sudo install -m 755 openlens-$V-$A-linux/openlens openlens-$V-$A-linux/openlens-cli /opt/openlens/$V/
sudo ln -sfn "$(readlink /opt/openlens/current)" /opt/openlens/previous
sudo ln -sfn $V /opt/openlens/current
sudo systemctl restart openlens
```

### Roll back

`openlens-update` does this on its own when a new release is not healthy. By hand:

```sh
sudo systemctl stop openlens
sudo ln -sfn "$(readlink /opt/openlens/previous)" /opt/openlens/current
sudo systemctl start openlens
```

If the log then says *this data was written by OpenLens X (data format N)*, the newer release changed the data format: put back the copy it made before it started, keeping the key files and the icons, which backups do not hold:

```sh
sudo systemctl stop openlens
sudo mv /var/lib/openlens /var/lib/openlens.newer
sudo cp -a "/var/lib/openlens.newer/backups/upgrade/$(ls /var/lib/openlens.newer/backups/upgrade | tail -n 1)" /var/lib/openlens
sudo cp -a /var/lib/openlens.newer/.secret.key /var/lib/openlens.newer/.session.key /var/lib/openlens.newer/icons /var/lib/openlens/ 2> /dev/null
sudo systemctl start openlens
```

Events received between the update and the rollback are in `/var/lib/openlens.newer` only.

## Backups

From the dashboard (*Projects → Storage and backups*), or a timer or cron line as the `openlens` user:

```sh
sudo -u openlens env OPENLENS_DATA_DIR=/var/lib/openlens /usr/local/bin/openlens backup --into /var/lib/openlens/backups/daily --keep 7
```

Then copy that directory off the machine. Details in [README.md](README.md#backups).
