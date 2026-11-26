# Deploy with Docker or Compose

The general guide is [README.md](README.md): requirements, release channels, updates, backups and settings.

## Run

```sh
docker run -d --name openlens --restart unless-stopped \
  -p 127.0.0.1:8686:8686 -v openlens-data:/data \
  -e OPENLENS_PUBLIC_URL=https://analytics.example.com \
  ghcr.io/philippe-arnd/openlens:0.6
docker logs openlens   # first start: a single-use owner link, valid 1 hour
```

Or with Compose (`compose.yaml`):

```yaml
services:
  openlens:
    image: ghcr.io/philippe-arnd/openlens:0.6
    restart: unless-stopped
    ports: ["127.0.0.1:8686:8686"]
    environment:
      OPENLENS_PUBLIC_URL: https://analytics.example.com
      # OPENLENS_ADMIN_TOKEN: pin it here, or read it from the first start's log
    volumes: [openlens-data:/data]
    healthcheck:
      test: ["CMD", "/usr/local/bin/openlens", "healthcheck"]
      interval: 15s
      timeout: 3s
      retries: 3
      start_period: 5s
volumes:
  openlens-data:
```

The image has no shell: the healthcheck uses the binary's own `healthcheck` subcommand. The image runs on `linux/amd64` and `linux/arm64` (a Raspberry Pi 4 or 5 with a 64-bit system, Ampere and Graviton servers): Docker picks the right one.

## HTTPS and reverse proxy

Publish the port on `127.0.0.1` only and put your proxy in front. OpenLens reads the client address from the last `X-Forwarded-For` entry, which is the one your proxy adds.

**Caddy** (automatic HTTPS):
```
analytics.example.com {
  reverse_proxy 127.0.0.1:8686
}
```

**Nginx:**
```nginx
server {
  listen 443 ssl;
  server_name analytics.example.com;
  # ssl_certificate … (certbot)
  client_max_body_size 10m;
  location / {
    proxy_pass http://127.0.0.1:8686;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
  }
}
```

**Traefik** (labels on the service, Compose provider):
```yaml
    labels:
      - traefik.enable=true
      - traefik.http.routers.openlens.rule=Host(`analytics.example.com`)
      - traefik.http.routers.openlens.tls.certresolver=letsencrypt
      - traefik.http.services.openlens.loadbalancer.server.port=8686
```

## Updates

Follow a channel: a minor (`:0.6` above; patches only, no behaviour change) or `:latest`. Never `:edge`, which follows `main`. On start, a new version backs up the data (`backups/upgrade/`) and refuses data from a newer release ([README.md](README.md#updates-without-anyone)).

**Hands-off, with a rollback:** [`deploy/docker/openlens-compose-update`](../../deploy/docker/openlens-compose-update) pulls the channel tag of the `openlens` service, recreates it, and waits up to 15 minutes for the healthcheck (the upgrade backup comes first). If the new image exits, restarts or never gets healthy, it goes back to the image that ran before and remembers the failed one, so later runs wait for the next image. With [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) installed, it checks the image signature before the switch ([Verify what you run](README.md#verify-what-you-run)). It needs the healthcheck of the Compose file above, and exits with an error when it rolled back, so cron mails you.

```sh
sudo curl -fsSL -o /usr/local/bin/openlens-compose-update \
  https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/docker/openlens-compose-update
sudo chmod 755 /usr/local/bin/openlens-compose-update
# crontab -e, daily at 04:17:
17 4 * * * /usr/local/bin/openlens-compose-update /srv/openlens
```

Plain `docker compose pull -q && docker compose up -d` from cron also works, without the rollback.

### Roll back

`openlens-compose-update` goes back to the previous image on its own. By hand: put the previous release in `image:` (for example `:v0.6.0`), then `docker compose up -d`.

If the log then says *this data was written by OpenLens X (data format N)*, the newer release changed the data format: put back the copy it made before it started. The image has no shell, so a throwaway BusyBox container does it; what the backup replaces goes to `/data/.newer`, and the keys, icons and other backups stay:

```sh
cd /srv/openlens && docker compose stop openlens
docker run --rm --volumes-from "$(docker compose ps -aq openlens)" busybox:1.37 sh -c '
  cd /data && b=$(ls -d backups/upgrade/* | tail -n 1) && mkdir -p .newer &&
  for t in wal events replay rollups sourcemaps cold meta.db meta.db-wal meta.db-shm openlens-version.json cold-manifest.json; do
    if [ -e "$t" ]; then mv "$t" .newer/; fi
  done && cp -a "$b"/. /data/'
docker compose up -d
```

## Backups

```sh
docker exec openlens openlens backup --into /data/backups/daily --keep 7
```

Run it from cron, then copy the volume's `backups/` off the machine (rsync, rclone, or the optional S3 copy in [README.md](README.md#optional-settings-d-016)).
