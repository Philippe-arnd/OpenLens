# Deploy on Nomad

The general guide is [README.md](README.md): requirements, release channels, backups and settings. The files are in [`deploy/nomad/`](../../deploy/nomad/):

| File | What it is |
| --- | --- |
| `openlens.nomad.hcl` | the job: one allocation, Docker driver, a host volume, a health check, reverted by Nomad on a failed update |
| `openlens-nomad-update` | moves the job to the image behind a channel tag, from cron |

OpenLens keeps its data on one local volume and is its only writer, so the job runs **one allocation on the client that has the volume**. Nomad brings the restart policy, the memory limit and reverted updates; it does not bring a second copy of the data (see [Backups](README.md#backups)).

## Run

On the client that keeps the data, declare a host volume owned by the image's user (uid 65532) in the client configuration, then restart the Nomad agent:

```sh
sudo install -d -o 65532 -g 65532 -m 750 /srv/openlens
```

```hcl
client {
  host_volume "openlens-data" {
    path      = "/srv/openlens"
    read_only = false
  }
}
```

Then run the job:

```sh
curl -fsSLO https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/nomad/openlens.nomad.hcl
nomad job run -var public_url=https://analytics.example.com openlens.nomad.hcl
nomad alloc logs -job openlens | grep -i owner   # first start: a single-use owner link, valid 1 hour
```

The job publishes port 8686 on that client: put your reverse proxy with HTTPS in front ([docker.md](docker.md#https-and-reverse-proxy)) and keep the port closed to the outside. The service is registered in Nomad's own service catalogue as `openlens`, for a proxy that reads it (Traefik's Nomad provider, or a `nomadService` template). The image is `linux/amd64` for now.

What the job sets:

- **One allocation at a time:** an update without canaries stops the old allocation before it places the new one, since two processes must never write the same data.
- **Reverted by Nomad:** if the new allocation does not pass its `/healthz` check, the deployment fails and Nomad goes back to the last healthy version of the job (`auto_revert`).
- **A long first start:** a new version first copies the data to `backups/upgrade/` before it listens, so the deployment waits up to 20 minutes and the check does not restart the task in its first 15.
- **Memory:** `memory = 1024` (MB), which covers about 2 M events in 90 days; 2048 up to about 4 M ([requirements](README.md#requirements)). OpenLens reads the limit and sizes its query memory to it.

Optional settings go in the task's `env` block ([the list](README.md#optional-settings-d-016)). Keep secrets such as `OPENLENS_SMTP_URL` out of the job file: put them in Nomad Variables (`nomad var put nomad/jobs/openlens OPENLENS_SMTP_URL=...`) and add a `template` block with `env = true` that reads them with `nomadVar`.

## Updates

The job follows a channel: a minor such as `:0.6` (patches only) or `:latest`, never `:edge`. Nomad does not look at a tag again by itself, so `openlens-nomad-update` asks GHCR for the digest behind the channel and runs the job on that digest. The same digest is no change: nothing restarts. A new one is a new version of the job, which Nomad rolls out and reverts on failure. With [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) installed, it checks the image signature first ([Verify what you run](README.md#verify-what-you-run)). It needs `curl`, `jq` and the `nomad` CLI with a token allowed to run the job (`NOMAD_ADDR`, `NOMAD_TOKEN`).

```sh
sudo curl -fsSL -o /usr/local/bin/openlens-nomad-update \
  https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/nomad/openlens-nomad-update
sudo chmod 755 /usr/local/bin/openlens-nomad-update
# crontab -e, daily at 04:17; the -var options are passed to `nomad job run`:
17 4 * * * /usr/local/bin/openlens-nomad-update /srv/nomad/openlens.nomad.hcl 0.6 -var public_url=https://analytics.example.com
```

It waits for the deployment and exits with an error when Nomad reverted it, so cron mails you. A reverted digest is remembered in `.openlens-failed-digests` next to the job file, and later runs wait for the next image.

### Roll back

By hand: `nomad job revert openlens <version>` (the versions are in `nomad job history openlens`), or `nomad job run -var image=ghcr.io/philippe-arnd/openlens:vX.Y.Z openlens.nomad.hcl` for a given release.

If the previous release then refuses the data (*this data was written by OpenLens X (data format N)*), the newer release changed the data format. Stop the job (`nomad job stop openlens`), put the upgrade backup back in `/srv/openlens` on that client with the steps of [systemd.md](systemd.md#roll-back) (the data directory is `/srv/openlens`, owned by 65532), then run the job again.

## Backups

From the dashboard (*Projects → Storage and backups*), or a periodic job or cron line on the client:

```sh
nomad alloc exec -job openlens /usr/local/bin/openlens backup --into /data/backups/daily --keep 7
```

Then copy `/srv/openlens/backups/daily` off the machine. Details in [README.md](README.md#backups).
