# Deploy on Coolify (one click, nothing to configure)

The general guide is [README.md](README.md): requirements, release channels, updates, backups and optional settings. This page only covers what is specific to Coolify.

On Coolify, the template maps two values that Coolify generates itself: the public URL and the admin token.

## Option A: template (image from GHCR)

1. In Coolify: **New Resource → Docker Compose (empty)**, then paste `deploy/coolify/openlens.yaml`.
2. Click **Deploy**. Coolify assigns a domain and generates `SERVICE_PASSWORD_64_OPENLENSADMIN`, which is the admin token.
3. Open the domain and log in with that token. Or let your agent call `POST /api/login-links` and send you the link.
   The first time, OpenLens asks for your email and creates your admin account, so the team list knows who you are (you can skip it). Invite others from **Team**.

The template follows `:latest`. For hands-off patch updates only, change the image tag to the minor channel (for example `:0.2`).

## Option B: build from the repository

**New Resource → Public Repository (or your fork through the GitHub App) → openlens → Build pack: Docker Compose → `/docker-compose.yml`**.

The root compose file already carries the healthcheck (Coolify reads healthchecks from the compose file, not from the Dockerfile). To pin the admin token, set `OPENLENS_ADMIN_TOKEN` in Coolify. If you don't, it is generated on first start and printed in the logs.

## Updates

**A restart does not pull the image again; a redeploy does.** To redeploy after each release, call Coolify's deploy API from the release workflow. In this repository, set the secrets `COOLIFY_DEPLOY_URL` (`https://<coolify>/api/v1/deploy?uuid=<service uuid>`) and `COOLIFY_TOKEN` (a token with the *deploy* ability). Then `release.yml` triggers it for every stable release, once the image has passed its smoke and upgrade tests.

## Backups

From Coolify's terminal, or as a Coolify scheduled task on the service:

```sh
openlens backup --into /data/backups/daily --keep 7
```

Then copy the directory off the machine (for example a scheduled task with rsync or rclone, or the optional S3 copy).

## Settings

Optional features are environment variables (list in [README.md](README.md#optional-settings-d-016)); set them in the service's **Environment Variables** tab. `OPENLENS_PUBLIC_URL` is not needed: Coolify provides `SERVICE_URL_OPENLENS`.
