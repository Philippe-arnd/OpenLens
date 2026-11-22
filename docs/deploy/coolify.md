# Deploy on Coolify (one click, nothing to configure)

OpenLens needs **no environment variables**. On Coolify, the template only maps two values that Coolify generates itself: the public URL and the admin token.

## Option A: template (image from GHCR)

1. In Coolify: **New Resource → Docker Compose (empty)**, then paste `deploy/coolify/openlens.yaml`.
2. Click **Deploy**. Coolify assigns a domain and generates `SERVICE_PASSWORD_64_OPENLENSADMIN`, which is the admin token.
3. Open the domain and log in with that token. Or let your agent call `POST /api/login-links` and send you the link.
   The first time, OpenLens asks for your email and creates your admin account, so the team list knows who you are (you can skip it). Invite others from **Team**.

The image `ghcr.io/philippe-arnd/openlens` is public, with these tags:

| Tag | What it follows |
| --- | --- |
| `:latest` | the newest stable release (the template's default) |
| `:0.2` | patches of one minor version: fixes, no behaviour change |
| `:vX.Y.Z` | one release, never moves |
| `:edge` | `main`, for testing only |

## Updates without anyone

On start, OpenLens backs up its data when the version changed. It refuses data written by a newer release, so an update can be undone by going back to the previous tag and restoring `backups/upgrade/`. With that safety net, let updates happen on their own:
- **Coolify redeploys pull the tag again.** To redeploy after each release, call Coolify's deploy API from the release workflow. In this repository, set the secrets `COOLIFY_DEPLOY_URL` (`https://<coolify>/api/v1/deploy?uuid=<service uuid>`) and `COOLIFY_TOKEN` (a token with the *deploy* ability). Then `release.yml` triggers it for every stable release, once the image has passed its smoke and upgrade tests.
- **Otherwise**, the dashboard footer says *Update available: vX.Y.Z*, from a daily check of GitHub's public releases API. Nothing about the install is sent. `OPENLENS_UPDATE_CHECK=off` disables the check.

## Option B: build from the repository

**New Resource → Public Repository (or your fork through the GitHub App) → openlens → Build pack: Docker Compose → `/docker-compose.yml`**.

The root compose file already carries the healthcheck (Coolify reads healthchecks from the compose file, not from the Dockerfile). To pin the admin token, set `OPENLENS_ADMIN_TOKEN` in Coolify. If you don't, it is generated on first start and printed in the logs.

## After the deploy: hand it to your agent

```
Connect <app> to OpenLens at https://<your-domain>.
Guide: https://<your-domain>/agents.md — admin token: <token>.
```

The agent creates the project, wires the SDK with roles, writes the feature map and sends you a login link. Data lives in the `openlens-data` volume, one directory holding the Parquet segments, the WAL, the replays and `meta.db`. Events are kept for 400 days and replays for 30 days (D-015).

**Backups.** `openlens backup` writes a consistent copy of that directory, and it is safe while the server runs. From Coolify's terminal or the host:

```sh
docker exec <container> openlens backup /data/backups/$(date +%F)
```

Then copy `/data/backups/<date>` off the machine (for example a Coolify scheduled task with rsync or rclone). **To restore:** stop the service, replace the volume's contents with the backup, then start it again. If the admin token came from `OPENLENS_ADMIN_TOKEN`, keep it set to the same value.

## Optional settings (D-016)

None of these is needed. Each feature stays off until its variables are set.

| Feature | Variables |
| --- | --- |
| Single sign-on (OpenID Connect). Register `<public url>/oidc/callback` as the redirect URI. Only invited emails (Team) can sign in. | `OPENLENS_OIDC_ISSUER`, `OPENLENS_OIDC_CLIENT_ID`, `OPENLENS_OIDC_CLIENT_SECRET` |
| Insight cards reworded in plain language by Claude (cached a day; originals kept as tooltips; a rewording that changes a number is discarded). Only the cards' text is sent. | `OPENLENS_LLM_API_KEY` (Anthropic API key), optional `OPENLENS_LLM_MODEL` (default `claude-opus-5-5`) |
| Second copy on S3-compatible storage (AWS, R2, MinIO, Garage): closed days of events and a daily `meta.db` snapshot, hourly. Local disk stays the source of truth. | `OPENLENS_S3_BUCKET`, `OPENLENS_S3_ACCESS_KEY_ID`, `OPENLENS_S3_SECRET_ACCESS_KEY`, optional `OPENLENS_S3_REGION`, `OPENLENS_S3_ENDPOINT` (non-AWS), `OPENLENS_S3_PREFIX` |
| Weekly digest by email (Mondays, 07:00 UTC) | `OPENLENS_SMTP_URL` (for example `smtps://user:pass@smtp.example.com:465`), `OPENLENS_DIGEST_TO` (comma-separated), optional `OPENLENS_DIGEST_FROM` |
| Sign-in by email ("Email me a login link" for invited users) | the SMTP settings above and the public URL (`OPENLENS_PUBLIC_URL`; Coolify sets `SERVICE_URL_OPENLENS` itself). Links work once, within 15 minutes, and are rate-limited |
| No daily check for a newer release (the footer's *Update available*) | `OPENLENS_UPDATE_CHECK=off` |

The digest is always visible in the dashboard (*This week vs last*), and agents can read it with the MCP `digest` tool.
