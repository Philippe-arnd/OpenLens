# Deploy on a hosting panel

Dokploy, CapRover, Dokku, Portainer, Unraid and TrueNAS all run the one Docker image. The general guide is [README.md](README.md): requirements, release channels, backups and settings. Coolify has [its own page](coolify.md). YunoHost is not covered: it needs an app package of its own.

Whatever the panel, the rules are those of [docker.md](docker.md):

- **The image:** `ghcr.io/philippe-arnd/openlens`, on a channel: a minor such as `:0.6` (patches only) or `:latest`, never `:edge`. Its platforms are listed in [README.md](README.md#image-and-release-channels).
- **One volume at `/data`,** writable by the image's user (uid 65532). A new named volume gets the right owner by itself; a host folder needs `chown 65532:65532`, or the container run as the folder's owner.
- **One instance.** OpenLens is the only writer of its data: never scale it above 1, and update it by stopping the old container before starting the new one, which is what a plain redeploy does.
- **Port 8686,** behind the panel's proxy with HTTPS. Set `OPENLENS_PUBLIC_URL` to the public address.
- **The first start** prints a single-use owner link, valid one hour, in the container's log. A new version also first copies the data to `backups/upgrade/` before it listens, so give it time on the first start after an update.
- **Updates:** a redeploy pulls the channel tag again. On start, a new version backs up its data and refuses data from a newer release; to go back, redeploy the previous release (`:vX.Y.Z`), and if it refuses the data, put back the upgrade backup as in [docker.md](docker.md#roll-back).
- **Backups:** *Projects → Storage and backups* in the dashboard, or `openlens backup --into /data/backups/daily --keep 7` from the panel's console or scheduled tasks, then copy the folder off the machine ([README.md](README.md#backups)).

The Compose file the panels below take:

```yaml
services:
  openlens:
    image: ghcr.io/philippe-arnd/openlens:0.6
    restart: unless-stopped
    environment:
      OPENLENS_PUBLIC_URL: https://analytics.example.com
    volumes:
      - openlens-data:/data
    healthcheck:
      test: ["CMD", "/usr/local/bin/openlens", "healthcheck"]
      interval: 15s
      timeout: 3s
      retries: 3
      start_period: 15m
volumes:
  openlens-data:
```

The healthcheck uses the binary's own `healthcheck` subcommand: the image has no shell.

## Dokploy

1. **Create Service → Compose**, type *Docker Compose*, provider *Raw*, and paste the file above.
2. **Domains:** add your domain for the service `openlens`, container port `8686`, with HTTPS (Let's Encrypt). Dokploy routes it through its Traefik.
3. **Deploy,** then read the owner link in **Logs**.

Updates: **Deploy** again pulls the channel tag. To redeploy on a schedule or from a release hook, call the service's deploy webhook (**Deployments → Webhook URL**) from cron.

## CapRover

[`deploy/caprover/openlens.yml`](../../deploy/caprover/openlens.yml) is a one-click app template:

1. **Apps → One-Click Apps/Databases → TEMPLATE** (the last entry of the list), paste the file, **Next**.
2. Name the app (for example `openlens`) and pick the channel (`0.6` by default). The public URL is set to `https://<app>.<your root domain>`.
3. In the app's **HTTP Settings**, **Enable HTTPS**, then **Force HTTPS**. Read the owner link in the app's **App Logs**.

Keep the app at one instance. Updates: **Deployment → Deploy via ImageName** with the same channel tag (`ghcr.io/philippe-arnd/openlens:0.6`) pulls it again; CapRover's Swarm service stops the old container first.

## Dokku

Dokku runs the image with `git:from-image`. Turn off its zero-downtime checks first, so a deploy stops the old container before it starts the new one:

```sh
dokku apps:create openlens
dokku checks:disable openlens
dokku storage:ensure-directory openlens
sudo chown 65532:65532 /var/lib/dokku/data/storage/openlens
dokku storage:mount openlens /var/lib/dokku/data/storage/openlens:/data
dokku config:set --no-restart openlens OPENLENS_PUBLIC_URL=https://analytics.example.com
dokku domains:set openlens analytics.example.com
dokku ports:set openlens http:80:8686
dokku git:from-image openlens ghcr.io/philippe-arnd/openlens:v0.6.0
dokku letsencrypt:enable openlens    # with the dokku-letsencrypt plugin
dokku logs openlens | grep -i owner
```

Dokku deploys an image when its name changes, so it follows releases rather than a channel: `dokku git:from-image openlens ghcr.io/philippe-arnd/openlens:v0.6.1` updates to a release, and the same command with the previous release goes back. Backups: `dokku enter openlens web /usr/local/bin/openlens backup --into /data/backups/daily --keep 7`.

## Portainer

1. **Stacks → Add stack**, build method **Web editor**, and paste the file above. Add `ports: ["127.0.0.1:8686:8686"]` to the service for a proxy on the same host, or attach it to your proxy's network.
2. **Deploy the stack,** then read the owner link in the container's **Logs**.

Updates: in the stack, **Update the stack** with **Re-pull image and redeploy** turned on. With the stack created from a Git repository instead of the web editor, Portainer's GitOps updates (polling or webhook) with **Re-pull image** do it on their own. On Portainer's Swarm environments, the same file runs as a stack: keep `replicas: 1`, as in [swarm.md](swarm.md).

## Unraid

[`deploy/unraid/openlens.xml`](../../deploy/unraid/openlens.xml) is a Docker template:

1. Copy it to the flash drive as `/boot/config/plugins/dockerMan/templates-user/my-openlens.xml`.
2. **Docker → Add Container**, template **openlens**. Set **Public URL**, check the data folder (`/mnt/user/appdata/openlens` by default), **Apply**.
3. Read the owner link in the container's log.

The container runs as `nobody:users` (`--user 99:100`), the owner of `appdata`, rather than the image's own user. Put Unraid's reverse proxy (for example Nginx Proxy Manager or SWAG) with HTTPS in front of port 8686. Updates: **Check for Updates**, then **apply update**, pulls the channel tag again.

## TrueNAS

On TrueNAS Community Edition 24.10 or later (apps run on Docker):

1. Create a dataset for the data, for example `tank/apps/openlens`, and give it to the image's user: `chown 65532:65532 /mnt/tank/apps/openlens`.
2. **Apps → Discover Apps → Custom App → Install via YAML**, and paste the file above with the volume replaced by the dataset (`- /mnt/tank/apps/openlens:/data`, and no `volumes:` section at the end) and the port published (`ports: ["8686:8686"]`).
3. Read the owner link in the app's logs, and put your reverse proxy with HTTPS in front.

Updates: the app's **Update** (or editing it and saving) pulls the channel tag again.
