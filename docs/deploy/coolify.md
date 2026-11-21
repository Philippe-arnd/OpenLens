# Deploy on Coolify (one click, nothing to configure)

OpenLens needs **no environment variables**. On Coolify, the template only maps two values that Coolify generates itself: the public URL and the admin token.

## Option A: template (image from GHCR)

1. In Coolify: **New Resource → Docker Compose (empty)**, then paste `deploy/coolify/openlens.yaml`.
2. Click **Deploy**. Coolify assigns a domain and generates `SERVICE_PASSWORD_64_OPENLENSADMIN`, which is the admin token.
3. Open the domain and log in with that token. Or let your agent call `POST /api/login-links` and send you the link.

The image `ghcr.io/philippe-arnd/openlens` is published from `main` by the release workflow. While the repository is private, the package is private too: either give Coolify a GHCR registry credential or use option B.

## Option B: build from the repository

**New Resource → Private Repository (GitHub App) → openlens → Build pack: Docker Compose → `/docker-compose.yml`**.

The root compose file already carries the healthcheck (Coolify reads healthchecks from the compose file, not from the Dockerfile). To pin the admin token, set `OPENLENS_ADMIN_TOKEN` in Coolify. If you don't, it is generated on first start and printed in the logs.

## After the deploy: hand it to your agent

```
Connect <app> to OpenLens at https://<your-domain>.
Guide: https://<your-domain>/agents.md — admin token: <token>.
```

The agent creates the project, wires the SDK with roles, writes the feature map and sends you a login link. Data lives in the `openlens-data` volume (one directory: Parquet segments plus `meta.db`), so backing it up means copying that volume.
