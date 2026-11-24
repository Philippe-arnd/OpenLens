# Deploy with Docker or Compose

The general guide is [README.md](README.md): requirements, release channels, updates, backups and settings.

## Run

```sh
docker run -d --name openlens --restart unless-stopped \
  -p 127.0.0.1:8686:8686 -v openlens-data:/data \
  -e OPENLENS_PUBLIC_URL=https://analytics.example.com \
  ghcr.io/philippe-arnd/openlens:0.2
docker logs openlens   # first start: a single-use owner link, valid 1 hour
```

Or with Compose (`compose.yaml`):

```yaml
services:
  openlens:
    image: ghcr.io/philippe-arnd/openlens:0.2
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

The image has no shell: the healthcheck uses the binary's own `healthcheck` subcommand. The image is `linux/amd64` for now; on ARM hosts use the [binary](binary.md) until the multi-arch image lands ([#238](https://github.com/Philippe-arnd/OpenLens/issues/238)).

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

Follow a minor channel (`:0.2`) and pull it on a schedule, for example a daily cron line:

```sh
cd /srv/openlens && docker compose pull -q && docker compose up -d
```

The upgrade backup and the data-format guard make this safe; to undo, pin the previous tag and restore `backups/upgrade/`. A built-in updater for Docker and the binary is tracked in [#238](https://github.com/Philippe-arnd/OpenLens/issues/238).

## Backups

```sh
docker exec openlens openlens backup --into /data/backups/daily --keep 7
```

Run it from cron, then copy the volume's `backups/` off the machine (rsync, rclone, or the optional S3 copy in [README.md](README.md#optional-settings-d-016)).
