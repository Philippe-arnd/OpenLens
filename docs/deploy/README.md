# Self-hosting OpenLens

OpenLens is **one binary and one data directory**. It needs **no environment variables**: the admin token and the capture keys generate themselves on first start. Pick the way you already run services.

| Where | Install | Guide |
| --- | --- | --- |
| **Docker / Compose** (any VPS) | one container, one volume | [docker.md](docker.md) |
| **Binary + systemd** (bare metal, VM, LXC, Raspberry Pi) | a `.tar.gz` from the releases, `x86_64` or `aarch64` | [binary.md](binary.md) |
| **Kubernetes** | Helm chart or Kustomize base | [kubernetes.md](kubernetes.md) |
| **Coolify** | one-click template | [coolify.md](coolify.md) |
| Other panels (Dokploy, CapRover, Portainer…), Nomad, Swarm, NixOS | use the Docker image and the compose file for now | [#238](https://github.com/Philippe-arnd/OpenLens/issues/238) |

## Requirements
- **CPU:** one vCPU is plenty for most apps (ingestion is about 80k events/s per vCPU).
- **Memory:** 512 MiB to 1 GiB, for up to about 4 M events in 90 days. Larger apps need more, and the board gets slow (D-023). See [kubernetes.md](kubernetes.md#sizing) for the details, which apply everywhere.
- **Disk:** about 11 bytes per event (events kept 400 days) plus replays (kept 30 days). Leave room for one extra copy of the data: it is made before each upgrade.
- **A public HTTPS URL** for the browser SDK, through the reverse proxy you already use (recipes in [docker.md](docker.md#https-and-reverse-proxy)). Set `OPENLENS_PUBLIC_URL` to it so login links and emails point to the right place.

## Image and release channels

The image `ghcr.io/philippe-arnd/openlens` is public. Binaries for `x86_64` and `aarch64` Linux are attached to each [GitHub release](https://github.com/Philippe-arnd/OpenLens/releases).

| Tag | What it follows |
| --- | --- |
| `:latest` | the newest stable release |
| `:0.2` | patches of one minor version: fixes, no behaviour change (recommended for hands-off updates) |
| `:vX.Y.Z` | one release, never moves |
| `:edge` | `main`, for testing only |

### Verify what you run

Images and release assets are signed in CI with Sigstore, keyless: no key to leak, the signature names the GitHub workflow that built them. Each release also carries an SPDX SBOM (`openlens-vX.Y.Z.spdx.json`), and the image carries its own.

```sh
# The image (any tag: the signature covers the digest behind it).
cosign verify ghcr.io/philippe-arnd/openlens:latest \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/Philippe-arnd/OpenLens/\.github/workflows/release\.yml@refs/(heads/main|tags/v.*)$'
# A downloaded binary, SDK bundle, chart or SBOM: its build provenance.
gh attestation verify openlens-vX.Y.Z-x86_64-linux.tar.gz -R Philippe-arnd/OpenLens
```

Releases before 0.6 are not signed. The provenance is SLSA Build level 2: the build runs on GitHub's hosted runners in the same workflow that publishes.

## Updates without anyone
- **Safety net, built in:** on start, OpenLens backs up its data when the version changed (`backups/upgrade/`). It refuses data written by a newer release, so going back to the previous tag (and restoring that copy) undoes an update.
- **Pulling new images is your platform's job:** Kubernetes and Coolify have recipes in their guides; for Docker and the binary, see their guides. Hands-off updates for every setup are tracked in [#238](https://github.com/Philippe-arnd/OpenLens/issues/238).
- **The dashboard footer** says *Update available: vX.Y.Z*, from a daily check of GitHub's public releases API. Nothing about the install is sent. `OPENLENS_UPDATE_CHECK=off` disables the check.

## Backups

`openlens backup` writes a consistent copy of the data directory, safe while the server runs. It holds events, WAL, replays, rollups, source maps and the cold-tier markers, plus `cold-manifest.json`: the days whose only copy is on S3 (D-031) and their S3 prefixes, so a restored server knows them at once. `openlens backup --into <dir> --keep 7` names the copy after the UTC time and keeps the newest seven: run it from cron, a systemd timer, a Kubernetes CronJob or your panel's scheduled tasks, then copy the directory off the machine (rsync, rclone, or the optional S3 copy below). **Or from the dashboard:** *Projects → Storage and backups* schedules daily or weekly backups at an hour (UTC), keeps the newest N in a folder (default `<data>/backups`) and optionally on S3 as `.tar.gz`, with a history, *Back up now* and a download of the latest copy. Failures are emailed to admins when SMTP is set. **To restore:** stop OpenLens, replace the data directory with the backup, start it again. If the admin token came from `OPENLENS_ADMIN_TOKEN`, keep the same value.

## Watching it run

- `GET /readyz`: `ready`, or 503 with the reason (metadata store, data volume not writable, volume almost full). The Helm chart and the Kustomize base use it as their readiness probe; liveness and startup stay on `/healthz` (a bare `ok`), so a full volume takes the pod out of the Service without restarting it.
- `GET /metrics`: Prometheus text, with the admin token or a `read` API token (`openlens-cli tokens create --scope read`): requests and events captured, rejected and rate-limited, writes refused for disk, flushes and their duration, age of the last flush, buffered events, WAL size, board computations and cache, SQL queries stopped at their time limit, WAL fsyncs (`wal_syncs_total`: under load, well below the capture requests), free disk.
- Slow clients: a connection gets 10 seconds to send its request headers, then it is closed (fixed, #346). Put a proxy with its own limits in front if your clients are slower.
- Every response carries `x-request-id` (the proxy's, when it sent one), also in the log line of the request.
- Ingestion requests time out after 10 s, the rest after 2 minutes (408).

## Optional settings (D-016)

None of these is needed. Each feature stays off until its variables are set.

| Feature | Variables |
| --- | --- |
| Where the server listens (default `0.0.0.0:8686`) and keeps its data (default `./data`; `/data` in the image) | `OPENLENS_ADDR`, `OPENLENS_DATA_DIR` |
| Choose the admin token yourself, instead of a generated one and a one-hour owner link in the first log (#342) | `OPENLENS_ADMIN_TOKEN` |
| Turn off email + password sign-in (#428), for installs that sign in with SSO only. On by default: after the first sign-in with the token, *My profile* creates your admin account (email + password), and the login page leads with it. **The admin token always works** on the login page (*Use the admin token*): it is the way back in after a lost password. An admin can also reset a member's password from *Team* (a one-time link). Each member can add an authenticator app as a second factor on *My profile → Sign-in* (#440); it is sealed with the same key as the S3 secret, so a store restored without that key loses the second factors | `OPENLENS_PASSWORD_LOGIN=off` |
| A project `default` with this capture key, created at start when there is no project yet (CI, demos, templates) | `OPENLENS_PROJECT_KEY` |
| *Create ticket* posts the drafted issue to GitHub (else it is stored encrypted from the ticket page, or you copy the Markdown) | `OPENLENS_GITHUB_TOKEN` |
| Single sign-on (OpenID Connect). Register `<public url>/oidc/callback` as the redirect URI. Only invited emails (Team) can sign in. | `OPENLENS_OIDC_ISSUER`, `OPENLENS_OIDC_CLIENT_ID`, `OPENLENS_OIDC_CLIENT_SECRET` |
| **Ask**, the AI chat (docs/ai.md): the model sees aggregates and definitions, never event properties or user ids unless an admin allows it per project. | `OPENLENS_LLM_PROVIDER` (`anthropic` or `openai-compatible`), `OPENLENS_LLM_BASE_URL`, `OPENLENS_LLM_API_KEY`, `OPENLENS_LLM_MODEL`, optional `OPENLENS_LLM_FAST_MODEL`, `OPENLENS_LLM_DAILY_TOKENS`; or Settings → AI |
| Insight cards reworded in plain language by Claude (cached a day; originals kept as tooltips; a rewording that changes a number is discarded). Only the cards' text is sent. | `OPENLENS_LLM_API_KEY` (Anthropic API key), optional `OPENLENS_LLM_MODEL` (default `claude-opus-5-5`) |
| Countries on the Audience page: the Docker image ships the DB-IP Lite country file. With the binary, download `dbip-country-lite-YYYY-MM.mmdb.gz` from db-ip.com into the data directory as `dbip-country-lite.mmdb.gz` (monthly), or point to it. The client address is only looked up, never stored. | `OPENLENS_GEOIP_DB` (optional path) |
| Alerts when the app is down (#256): set per project in *Settings → Alerts* (probe, traffic stopped, email, webhook, ntfy). Email goes to this list, else `OPENLENS_DIGEST_TO`, else the admins (needs `OPENLENS_SMTP_URL`). | `OPENLENS_ALERT_TO` (optional, comma-separated) |
| Second copy on S3-compatible storage (AWS, R2, MinIO, Garage): closed days of events, their daily rollups (kept after the events, D-026) and a daily `meta.db` snapshot, hourly. **Optional cold tier** (off by default, `OPENLENS_S3_COLD=on` or the Storage page): event days older than 120 days then leave the disk once S3 holds them, verified (D-031), and S3 becomes their only copy. OpenLens refuses to move any day if a bucket lifecycle rule expires objects in under 400 days or the rules cannot be read (D-032; the key needs `s3:GetLifecycleConfiguration`). Without the tier, local disk keeps every day. | `OPENLENS_S3_BUCKET`, `OPENLENS_S3_ACCESS_KEY_ID`, `OPENLENS_S3_SECRET_ACCESS_KEY`, optional `OPENLENS_S3_REGION`, `OPENLENS_S3_ENDPOINT` (non-AWS), `OPENLENS_S3_PREFIX` — or set it in *Storage and backups* (the variables win). The secret is sealed with `OPENLENS_SECRET_KEY` (64 hex characters) when set, else a key file made in the data directory (`.secret.key`, not in backups) |
| Weekly digest by email (Mondays, 07:00 UTC) | `OPENLENS_SMTP_URL` (for example `smtps://user:pass@smtp.example.com:465`), `OPENLENS_DIGEST_TO` (comma-separated), optional `OPENLENS_DIGEST_FROM` |
| Sign-in by email ("Email me a login link" for invited users) | the SMTP settings above and the public URL (`OPENLENS_PUBLIC_URL`; Coolify sets it itself). Links work once, within 15 minutes, and are rate-limited |
| No daily check for a newer release (the footer's *Update available*) | `OPENLENS_UPDATE_CHECK=off` |

The digest is always visible in the dashboard (*This week vs last*), and agents can read it with the MCP `digest` tool.

## After the deploy: hand it to your agent

```
Connect <app> to OpenLens at https://<your-domain>.
Guide: https://<your-domain>/agents.md — admin token: <token>.
```

The agent creates the project, wires the SDK with roles, writes the feature map and sends you a login link.

## Environments

Every event carries an environment, and the board shows production by default.

- **Detected:** the browser SDK reads the page's host. `localhost`, `*.local` and `*.test` are `development`. A host with `staging`, `stage` or `stg` is `staging`. `preview`, `review`, `pr-<n>`, `*.vercel.app` and `*.netlify.app` are `preview`.
- **Unknown, not production:** a host that no pattern and no rule matches is stored as `unknown` and kept out of production numbers (D-034). `openlens-cli verify` lists these hosts.
- **Say which one it is:** `openlens-cli env rule kanban-test.example.com staging`, or `*.preview.example.com preview`. `ignore` drops a host's events entirely, for example your developers' machines. The rule applies to the next events, with no change to the app. `openlens-cli env list` shows the hosts per environment, and agents use the MCP `environments` tool.
- **Set in the app:** `init({ environment })`, or the server SDKs' option or `OPENLENS_ENVIRONMENT`, always wins over the rules.
- **Aliases:** `openlens-cli env alias live-eu production` stores what the app sends as `live-eu` under `production`, on top of the usual folding (`prod`, `stage`, `dev`, …). `openlens-cli env alias sandbox ignore` drops it. Like rules, aliases apply to the next events.
- **Hide:** `openlens-cli env hide qa` takes `qa` out of the environment selector. Its events stay stored and readable by name (`?env=qa`, SQL, the API). `env show qa` brings it back. Production cannot be hidden.
- **Checks per environment:** production is checked at the project's app address and gets every alert. `openlens-cli env monitor staging https://staging.example.com` checks staging every minute too (same health path, certificate daily). At the default level, `log`, its incidents are only recorded on Uptime. With `--level notify` they are sent on the project's alert channels like production's.

All of this is also in *Settings → Environments*, `PATCH /api/projects/{id}` and the MCP `environments` tool. The browser SDK sends `unknown` for a host no pattern matches, and older SDKs send `production`: the server reads both the same way.

After an upgrade, every host that sent production events in the last 30 days gets a `production` rule, so nothing moves.
