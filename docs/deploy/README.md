# Self-hosting OpenLens

OpenLens is **one binary and one data directory**. It needs **no environment variables**: the admin token and the capture keys generate themselves on first start. Pick the way you already run services.

| Where | Install | Guide |
| --- | --- | --- |
| **Docker / Compose** (any VPS) | one container, one volume | [docker.md](docker.md) |
| **Binary + systemd** (bare metal, VM, LXC, Raspberry Pi) | a `.tar.gz` from the releases, `x86_64` or `aarch64`, with a hardened unit; `install.sh` does it in one command | [systemd.md](systemd.md) |
| **Kubernetes** | Helm chart or Kustomize base | [kubernetes.md](kubernetes.md) |
| **Coolify** | one-click template | [coolify.md](coolify.md) |
| **Docker Swarm** | a stack file, one replica | [swarm.md](swarm.md) |
| **Nomad** | a job with one allocation and a host volume | [nomad.md](nomad.md) |
| **NixOS** | a flake and the module `services.openlens` | [nixos.md](nixos.md) |
| **Dokploy, CapRover, Dokku, Portainer, Unraid, TrueNAS** | the Docker image; a template for CapRover and Unraid | [panels.md](panels.md) |
| Not covered yet: YunoHost | needs an app package of its own: run the Docker image with the settings of [docker.md](docker.md) | [#487](https://github.com/Philippe-arnd/OpenLens/issues/487) |

The Nomad job, the NixOS module and the panel recipes follow each tool's public documentation; they are not yet tested on the tools themselves in CI ([#486](https://github.com/Philippe-arnd/OpenLens/issues/486)).

## Requirements
- **CPU:** one vCPU is plenty for most apps (ingestion is about 80k events/s per vCPU).
- **Memory:** 1 GiB for up to about 2 M events in 90 days, 2 GiB up to about 4 M (measured: the board's first load takes about 1 s at 1 M, 2.5 s at 2 M, 3 s at 4 M; [benchmarks](../benchmarks.md#at-scale-under-a-memory-cap-460-2026-11-26)). These figures were measured while the board kept the 90-day window in memory, which it no longer does (D-051); the `scale` job will measure them again, larger apps included. See [kubernetes.md](kubernetes.md#sizing) for the details, which apply everywhere.
- **Disk:** about 11 bytes per event (events kept 400 days) plus replays (kept 30 days). Leave room for one extra copy of the data: it is made before each upgrade.
- **A public HTTPS URL** for the browser SDK, through the reverse proxy you already use (recipes in [docker.md](docker.md#https-and-reverse-proxy)). Set `OPENLENS_PUBLIC_URL` to it so login links and emails point to the right place.

## Image and release channels

The image `ghcr.io/philippe-arnd/openlens` is public, for `linux/amd64` and `linux/arm64` (one tag, both platforms, from 0.6 on; older releases are `linux/amd64` only). Binaries for `x86_64` and `aarch64` Linux are attached to each [GitHub release](https://github.com/Philippe-arnd/OpenLens/releases).

| Tag | What it follows |
| --- | --- |
| `:latest` | the newest stable release |
| `:0.6` | patches of one minor version: fixes, no behaviour change (recommended for hands-off updates) |
| `:0` | every release of one major version: before 1.0 the same as `:latest`; from 1.0 on, `:1` follows 1.x without breaking changes |
| `:vX.Y.Z` | one release, never moves |
| `:edge` | `main`, for testing only |

### Verify what you run

Images and release assets are signed in CI with Sigstore, keyless: no key to leak, the signature names the GitHub workflow that built them. Each release also carries an SPDX SBOM (`openlens-vX.Y.Z.spdx.json`), and the image carries its own.

```sh
# The image (any tag: the signature covers the digest behind it, and each platform's image).
cosign verify ghcr.io/philippe-arnd/openlens:latest \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/Philippe-arnd/OpenLens/\.github/workflows/release\.yml@refs/(heads/main|tags/v.*)$'
# A downloaded binary, SDK bundle, chart or SBOM: its build provenance.
gh attestation verify openlens-vX.Y.Z-x86_64-linux.tar.gz -R Philippe-arnd/OpenLens
```

Releases before 0.6 are not signed. The provenance above is SLSA Build level 2: it is made by the same workflow that builds and publishes.

**SLSA Build level 3.** Each release also carries `openlens-vX.Y.Z.intoto.jsonl`, and each image a second provenance, made by the [SLSA GitHub generator](https://github.com/slsa-framework/slsa-github-generator)'s reusable workflows. They run apart from the release workflow, with their own identity, and receive only the hashes of what was built, so the build cannot forge them. Check them with [`slsa-verifier`](https://github.com/slsa-framework/slsa-verifier); it prints the builder that signed (`generator_generic_slsa3.yml` or `generator_container_slsa3.yml`, at a release tag of the generator):

```sh
# A downloaded asset (binary, SDK bundle, chart, SBOM or SHA256SUMS), with the release's provenance file.
slsa-verifier verify-artifact openlens-vX.Y.Z-x86_64-linux.tar.gz \
  --provenance-path openlens-vX.Y.Z.intoto.jsonl \
  --source-uri github.com/Philippe-arnd/OpenLens
# The image, by digest (slsa-verifier refuses a tag).
DIGEST=$(docker buildx imagetools inspect ghcr.io/philippe-arnd/openlens:latest --format '{{json .Manifest}}' | jq -r .digest)
slsa-verifier verify-image "ghcr.io/philippe-arnd/openlens@$DIGEST" \
  --source-uri github.com/Philippe-arnd/OpenLens
```

The provenance records the ref the workflow ran on: `main` for build releases and for releases cut by the release train (it starts the release workflow from `main`), the tag for a tag pushed by hand and for the image of a tag. `--source-branch main` or `--source-tag vX.Y.Z` checks that too. Build releases before this change have no level 3 provenance; 0.6.0 is the first release with it.

## Updates without anyone
- **Safety net, built in:** on start, OpenLens backs up its data when the version changed (`backups/upgrade/`). It refuses data written by a newer release, so going back to the previous tag (and restoring that copy) undoes an update.
- **Following a channel, with a rollback when the new version is not healthy:**
  - binary + systemd: `openlens-update` and its daily timer check the checksum and the build provenance before installing, and go back to the previous release (and if needed to the upgrade backup) when the new one is not healthy ([systemd.md](systemd.md#hands-off-updates));
  - Docker Compose: `openlens-compose-update` from cron, back to the previous image on failure ([docker.md](docker.md#updates));
  - Swarm: `docker service update --image` from cron, rolled back by Swarm itself ([swarm.md](swarm.md#updates));
  - Nomad: `openlens-nomad-update` from cron runs the job on the digest behind the channel, reverted by Nomad on failure ([nomad.md](nomad.md#updates));
  - NixOS: the system's own upgrades, with the release tag pinned in the flake; `nixos-rebuild switch --rollback` goes back ([nixos.md](nixos.md#updates));
  - Kubernetes: the chart's opt-in `autoUpdate` CronJob restarts the pod on its channel tag (no automatic rollback), or Argo CD Image Updater / Flux bump the tag in Git ([kubernetes.md](kubernetes.md#hands-off-updates));
  - Coolify: the template follows the minor channel; a redeploy pulls it ([coolify.md](coolify.md#updates));
  - other panels: a redeploy pulls the channel tag again ([panels.md](panels.md)).

  Each one follows `:latest` or a minor channel, never `:edge`. When the previous version refuses the data (a new release changed the data format), only `openlens-update` restores the upgrade backup by itself; the guides say how to do it by hand elsewhere.
- **The dashboard footer** says *Update available: vX.Y.Z*, from a daily check of GitHub's public releases API. Nothing about the install is sent. `OPENLENS_UPDATE_CHECK=off` disables the check.

## Backups

`openlens backup` writes a consistent copy of the data directory, safe while the server runs. It holds `meta.db` (projects, team, settings, sealed secrets), events, WAL, replays, rollups, source maps, project icons and the cold-tier markers, plus `cold-manifest.json`: the days whose only copy is on S3 (D-031) and their S3 prefixes, so a restored server knows them at once. `openlens backup --into <dir> --keep 7` names the copy after the UTC time and keeps the newest seven: run it from cron, a systemd timer, a Kubernetes CronJob or your panel's scheduled tasks, then copy the directory off the machine (rsync, rclone, or the optional S3 copy below). **Or from the dashboard:** *Projects → Storage and backups* schedules daily or weekly backups at an hour (UTC), keeps the newest N in a folder (default `<data>/backups`) and optionally on S3 as `.tar.gz`, with a history, *Back up now* and a download of the latest copy. Failures are emailed to admins when SMTP is set. **To restore:** stop OpenLens, replace the data directory with the backup, start it again. If the admin token came from `OPENLENS_ADMIN_TOKEN`, keep the same value.

**Not in a backup, on purpose:** the two key files at the root of the data directory, `.secret.key` (seals the S3 secret and the authenticator-app second factors) and `.session.key` (signs session cookies), so a stolen backup alone cannot open sessions or unseal secrets. When you restore, copy both files from the old data directory into the restored one (or keep them in your password manager or secret store and put them back). If you set `OPENLENS_SECRET_KEY`, keep the same value; the file is then not used. Restored without them, OpenLens makes new keys at start: everyone signs in again (new session key), and with a new secret key the S3 secret must be entered again on *Storage and backups* and members who used an authenticator app must set it up again (an admin resets their password).

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

## Already on OpenTelemetry

An app that exports OpenTelemetry metrics can send its server route stats straight to OpenLens: `POST /v1/metrics` (OTLP/HTTP, protobuf or JSON) with the project key in `x-openlens-key` (or `Authorization: Bearer`). Point a Collector's `otlp_http` exporter at `https://<your-domain>` (it appends `/v1/metrics`), or set `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=https://<your-domain>/v1/metrics` and `OTEL_EXPORTER_OTLP_METRICS_HEADERS=x-openlens-key=<key>` on an SDK. Recipes and limits: [Server SDKs](../../sdk/SERVER.md#already-on-opentelemetry). Your reverse proxy must pass `/v1/metrics` and bodies up to 4 MB (Nginx's default `client_max_body_size` is 1 MB). gRPC (port 4317) is not served.

## A status page on its own host name

A project's public status page (Settings → Sharing, #561) is served at `/status/<slug>`. To show it at the root of a host such as `status.example.com`, point that host at OpenLens and rewrite `/` to the page in the reverse proxy, keeping `/status/` and `/assets/` as they are. With Caddy:

```
status.example.com {
    rewrite / /status/my-app
    reverse_proxy openlens:8686
}
```

With Nginx: `location = / { proxy_pass http://openlens:8686/status/my-app; }` and `location / { proxy_pass http://openlens:8686; }`. Only the status page and its assets need to be reachable on that host.

## Job check-ins behind a proxy

Cron and background jobs check in at `POST /check-in/<project-key>/<slug>` (or `POST /check-in/<slug>` with the key in `X-OpenLens-Key`, which the server SDKs use; see `sdk/SERVER.md`). OpenLens logs that path as `/check-in/<key>/<slug>`, never the key. A reverse proxy that writes access logs records the full path: either keep `/check-in/` out of its logs, or have scripts send the header form. The route takes no body and answers within 10 seconds.

## Environments

Every event carries an environment, and the board shows production by default.

- **Detected:** the browser SDK reads the page's host. `localhost`, `*.local` and `*.test` are `development`. A host with `staging`, `stage` or `stg` is `staging`. `preview`, `review`, `pr-<n>`, `*.vercel.app` and `*.netlify.app` are `preview`.
- **Unknown, not production:** a host that no pattern and no rule matches is stored as `unknown` and kept out of production numbers (D-034). `openlens-cli verify` lists these hosts.
- **Say which one it is:** `openlens-cli env rule kanban-test.example.com staging`, or `*.preview.example.com preview`. `ignore` drops a host's events entirely, for example your developers' machines. The rule applies to the next events, with no change to the app. `openlens-cli env list` shows the hosts per environment, and agents use the MCP `environments` tool.
- **Past events:** a rule applies to new events. After adding or changing one, `openlens-cli env relabel kanban-test.example.com --dry-run` counts how many of that host's events of the last 30 days (`--days`, at most 90) would move, per old and new environment; without `--dry-run` it rewrites them to what the rules give now (audited as `environments.relabel`). The same is on *Settings → Environments* (preview, then confirm), `POST /api/projects/{id}/environments` and the MCP `environments` tool (`relabel`, a dry run unless `dry_run` is false). Bots stay bots and a host the rules `ignore` keeps its events; an environment the app set itself on that host moves too, so read the preview.
- **Allowed hosts:** `openlens-cli env allow app.example.com '*.preview.example.com'` shows what this list would have dropped over the last 24 hours (hosts and events); add `--confirm` to save it. Events from other hosts are then dropped at ingestion, but only while a host on the list has sent events in the last 7 days: until then the list is *pending* and drops nothing, so a typo cannot lock the app out. Dropped events are counted per host on *Settings → Environments* and in `env list`. Events without a page address (server SDKs) always pass. `openlens-cli env allow --confirm` with no host turns it off (D-043).
- **Set in the app:** `init({ environment })`, or the server SDKs' option or `OPENLENS_ENVIRONMENT`, always wins over the rules.
- **Aliases:** `openlens-cli env alias live-eu production` stores what the app sends as `live-eu` under `production`, on top of the usual folding (`prod`, `stage`, `dev`, …). `openlens-cli env alias sandbox ignore` drops it. Like rules, aliases apply to the next events.
- **Hide:** `openlens-cli env hide qa` takes `qa` out of the environment selector. Its events stay stored and readable by name (`?env=qa`, SQL, the API). `env show qa` brings it back. Production cannot be hidden.
- **Checks per environment:** production is checked at the project's app address and gets every alert. `openlens-cli env monitor staging https://staging.example.com` checks staging every minute too (same health path, certificate daily). At the default level, `log`, its incidents are only recorded on Uptime. With `--level notify` they are sent on the project's alert channels like production's.

All of this is also in *Settings → Environments*, `PATCH /api/projects/{id}` and the MCP `environments` tool. The browser SDK sends `unknown` for a host no pattern matches, and older SDKs send `production`: the server reads both the same way.

After an upgrade, every host that sent production events in the last 30 days gets a `production` rule, so nothing moves.
