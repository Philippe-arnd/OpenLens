# Deploying OpenLens on Kubernetes (agent guide)

You are the agent that runs infrastructure for a team. The goal is the same as on Coolify: a running OpenLens, an app wired to it, and a login link for the PM, **with no value to invent and no question to ask**. Every step below has a check; do not move on until it passes.

**What you deploy.** One pod running one binary: API, ingestion, dashboard and storage. It needs:
- a persistent volume, holding the write-ahead log, the Parquet files and SQLite;
- an admin token.

There is no database, queue or cache to run next to it.

**One replica, by design.** OpenLens has a single writer: the WAL and SQLite live on one volume. The chart refuses `replicaCount` other than 1. Availability comes from fast restarts (the WAL is replayed on start) and from backups, not from replicas. An outage of the pod delays nothing for good: the SDK keeps events and retries.

## 1. Install

**Helm** (recommended):

```sh
# from v0.2.0, the chart is published with each release:
helm install openlens oci://ghcr.io/philippe-arnd/charts/openlens --namespace openlens --create-namespace
# until then, or to follow main: from a checkout of the repository
helm install openlens ./deploy/helm/openlens --namespace openlens --create-namespace
```

No value is required. The common ones:

| Value | Default | When to set it |
| --- | --- | --- |
| `ingress.enabled`, `ingress.host`, `ingress.className` | off | to expose the dashboard and the capture endpoint (the SDK needs a public URL) |
| `ingress.tls`, `ingress.annotations` | off | HTTPS; with cert-manager, add `cert-manager.io/cluster-issuer: <issuer>` |
| `persistence.size`, `persistence.storageClass` | `10Gi`, cluster default | see *Sizing* below |
| `image.tag` | the chart's release | to pin or test another release |
| `autoUpdate.enabled`, `autoUpdate.schedule` | off, `47 4 * * *` | hands-off updates within a channel (step 6) |
| `adminToken.existingSecret` | generated | to bring your own token Secret |
| `extraEnvFrom` | none | optional features (step 5) |

**Kustomize / plain manifests** (no Helm): `kubectl apply -k deploy/k8s/base -n openlens`. The base is rendered from the chart, so it behaves the same. `deploy/k8s/example` shows an overlay with a namespace, a pinned image and an Ingress on your host (the `components/ingress` component).

**Check:** `kubectl -n openlens rollout status statefulset/openlens` succeeds. With Helm, `helm test openlens -n openlens` passes; it probes `/healthz` through the Service.

## 2. Get the admin token

- **Helm** generates it into a Secret and keeps it across upgrades and uninstalls:

  ```sh
  kubectl -n openlens get secret openlens-admin -o jsonpath='{.data.admin-token}' | base64 -d; echo
  ```

  (The Secret is `<release>-openlens-admin` when the release name does not contain `openlens`.)
- **Kustomize:** create the Secret before the first start, so you know the token:

  ```sh
  kubectl -n openlens create secret generic openlens-admin --from-literal=admin-token="adm_$(openssl rand -hex 24)"
  ```

  If you don't, OpenLens generates a token and stores only its hash in its volume. The token itself is never shown: the log prints a single-use owner link valid 1 hour (`generated admin token … owner_link=`). Open it, then create API tokens for the CLI and agents in *Settings → API tokens*. Missed the hour, or lost access: set `OPENLENS_ADMIN_TOKEN` to choose a token, and restart.

**Never** put the token in values files, manifests or the app's repository. It belongs in a Secret, and in the agent's own secret store.

## 3. Reach it

| From | URL |
| --- | --- |
| Browsers and the SDK | `https://<ingress.host>` |
| An agent or job inside the cluster | `http://openlens.openlens.svc:8686` (from v0.2, the CLI accepts plain HTTP for cluster-internal names; v0.1 needs `https` or a port-forward to `localhost`) |
| Your machine, without an Ingress | `kubectl -n openlens port-forward svc/openlens 8686:8686` → `http://localhost:8686` |

**Check:** `curl -fsS <url>/healthz` prints `ok`.

## 4. Wire the app

Set `OPENLENS_SERVER` to the URL above and `OPENLENS_TOKEN` to the token. From there, follow the wiring guide exactly as on any other host: `<url>/agents.md` (the same as [docs/agents/wire.md](../agents/wire.md)). It covers:
1. `openlens-cli init`;
2. the SDK, with the public Ingress URL as its host;
3. identify;
4. the feature map;
5. backfill;
6. `verify`;
7. the login link for the PM.

The CLI is in the image, so an agent with cluster access can also run it there:

```sh
kubectl -n openlens exec statefulset/openlens -- openlens-cli --server http://localhost:8686 --token "$TOKEN" verify --project <id>
```

On the first login, the PM is asked for their email, and their admin account is created.

## 5. Optional features

Each feature is off until its variables are set ([docs/deploy/coolify.md](coolify.md) lists them):
- the SMTP weekly email;
- LLM rewording of cards;
- OpenID Connect single sign-on;
- an S3 copy.

Put the variables in a Secret, and reference it:

```sh
kubectl -n openlens create secret generic openlens-options \
  --from-literal=OPENLENS_SMTP_URL='smtps://user:pass@smtp.example.com:465' \
  --from-literal=OPENLENS_DIGEST_TO='pm@example.com' \
  --from-literal=OPENLENS_DIGEST_FROM='openlens@example.com'
helm upgrade openlens <chart> -n openlens --reuse-values \
  --set 'extraEnvFrom[0].secretRef.name=openlens-options'
```

## 6. Upgrade

- **Helm:** `helm upgrade openlens <chart> -n openlens --reuse-values`, with a new chart version, or `--set image.tag=vX.Y.Z`.
- **Kustomize:** change `images[].newTag` in your overlay and apply.

The pod restarts with the same volume and token. When the version changes, OpenLens first copies the data to `backups/upgrade/` on the volume, keeping only the latest copy, so leave room for one copy of the data. An older image refuses to start on data written by a newer one: roll back by restoring that copy. **Check:** `rollout status`, then `sql` (`SELECT count(*) FROM events`) returns at least the count from before. CI runs this exact upgrade on a kind cluster for every chart change.

Read the [CHANGELOG](../../CHANGELOG.md) section of the target version first: its **Changed** entries say what behaves differently.

### Hands-off updates

**Helm, opt-in:** `--set autoUpdate.enabled=true` adds a CronJob that runs `kubectl rollout restart` on the StatefulSet every day (04:47 UTC by default, `autoUpdate.schedule`), with the image pull policy set to `Always`, so the new pod pulls its tag again.

- **It follows a channel.** With `image.tag` empty, the image becomes the chart's minor channel (`:0.5` for chart 0.5.x): patches only, no behaviour change. `image.tag=latest` follows every stable release. A pinned release (`v0.5.1`), a digest or `edge` is refused at install, since a restart would bring nothing (or `main`).
- **Each run restarts the pod**, even when the tag has not moved: one replica on one volume, so the old pod stops before the new one starts. Expect a few seconds without the dashboard each day; the SDK keeps the events and retries. Pick a quiet hour.
- **Minimal rights.** The Job runs `registry.k8s.io/kubectl` under its own ServiceAccount, whose Role can only read and restart this StatefulSet. The OpenLens pod still gets no service-account token.
- **No automatic rollback.** The Job waits up to 20 minutes for the new pod to be ready (the upgrade backup comes first) and fails if it is not: watch failed Jobs (`kubectl -n openlens get jobs`). To go back, pin the previous release with `--set autoUpdate.enabled=false,image.tag=vX.Y.Z` (a pinned release with `autoUpdate` on is refused), and restore the upgrade backup if that release refuses the data (step 7).
- **Moving to the next minor** is a chart upgrade, after reading its **Changed** section.

**Argo CD or Flux:** let them update the image instead of the CronJob, so Git stays the source of truth and a bad image is reverted by a commit. Leave `autoUpdate` off and pin `image.tag` to a release; the tool bumps it within a range:

- [Argo CD Image Updater](https://argocd-image-updater.readthedocs.io/): follow `ghcr.io/philippe-arnd/openlens` with the `semver` strategy and the constraint `~0.5`, and write the result to the Helm parameter `image.tag` (back to the Application, or to Git with the `git` write-back method).
- [Flux image automation](https://fluxcd.io/flux/guides/image-update/): an `ImageRepository` on `ghcr.io/philippe-arnd/openlens`, an `ImagePolicy` with `semver: {range: "~0.5"}`, and an `ImageUpdateAutomation` that commits the new tag; mark the `image.tag` line of your HelmRelease values with `# {"$imagepolicy": "<namespace>:openlens:tag"}`.

Both read the `vX.Y.Z` tags, which never move, so each update is one recorded change. Their notifications (Argo CD Notifications, Flux's `Alert`) tell you when a sync or a health check fails.

**Kustomize:** no updater ships with the plain manifests; bump `images[].newTag` from your GitOps tool as above.

## 7. Back up and restore

`openlens backup` writes a consistent copy while the server runs.

**Scheduled (Helm, from v0.2):** `--set backup.enabled=true` adds a CronJob, every night at 03:17 UTC by default. It runs `openlens backup --into <dir> --keep 3` next to the server pod (the data volume is ReadWriteOnce). Each copy is named after the UTC time, and only the newest `keep` are kept.
- **Default target:** `backups/` on the data volume. Each copy takes the size of the data, so leave room for it. This protects against mistakes and corruption, not against losing the volume.
- **To keep the copies elsewhere:**
  - `backup.existingClaim` mounts another PVC at `/backups`, on the same node;
  - or enable the S3 copy (step 5);
  - or use your volume snapshots.
- **Check:** `kubectl -n openlens create job backup-now --from=cronjob/openlens-backup`, then `kubectl -n openlens logs job/backup-now` prints `"ok":true`.

**By hand:**

```sh
kubectl -n openlens exec statefulset/openlens -- openlens backup --into /data/backups --keep 3
kubectl -n openlens cp openlens-0:/data/backups/<copy> ./openlens-backup
```

**Restore:**
1. `kubectl -n openlens scale statefulset/openlens --replicas=0`.
2. Mount the PVC in a helper pod and replace its contents with the copy: `wal/`, `events/`, `replay/` and `meta.db`.
3. Scale back to 1.

Keep the same admin token Secret.

## Sizing

- **Disk:** about **11 bytes per event** after compression, and events are kept 400 days. So 1 M events a day needs about 4.5 GB a year. Replays (kept 30 days) often weigh more than events: watch the PVC usage during the first week and size from it. Leave room for one extra copy of the data, made before each upgrade.
- **CPU:** about **80k events/s per vCPU** at ingestion. The default request (`100m`) is enough for most apps.
- **Memory:** the default limit, **1 GiB for up to about 4 M events in 90 days.** Measured under a 1 GiB cap with the realistic profile: peak memory 350–390 MB at 1 M, 490–510 MB at 2 M, 650–790 MB at 4 M; the board is computed ahead, so it opens at once, and computing it takes 1.3–4.3 s at 1 M and 3–7 s at 4 M ([benchmarks](../benchmarks.md#footprint-targets-d-097)). The query memory stops at 768 MiB whatever the host (D-097), so a larger limit no longer speeds up the board: above 4 M events, send fewer low-value events rather than more memory. The board keeps no copy of the events (D-051): closed days come from daily rollups and every other query reads its own window from the files. The `scale` job of `perf.yml` measures 4 M under 1 GiB every night.

### What grows with the disk

OpenLens sizes what it keeps from its data volume, with no setting (D-088). Its budget is 80 % of the volume minus what other files on it use; after its own other files (rollups, `meta.db`, backups), half of the rest is for events and recordings and the other half for the copy made before each upgrade. Every hour it measures what each project's events take a day and what one of its recordings takes. Two things follow the room: **the number of recordings kept per project a day**, and **the length of the route-stats window** (D-091).

| What | On a small disk | On a larger disk |
| --- | --- | --- |
| Recordings kept per project a day | 300 (the floor) | more, the same number in every project, as many as fit for 30 days once events and the floor's recordings are paid for, up to 30 000 |
| Event retention | 400 days | 400 days: fixed ([privacy](../privacy.md), D-015) |
| Replay retention | 30 days | 30 days: fixed ([privacy](../privacy.md), D-015) |
| Route-stats window (server SDKs) | 10 minutes | 5 or 1 minute when their extra points fit for 400 days, paid before extra recordings; never finer than 10-minute windows of 500 keys would store a day, so a busy project with many routes keeps a coarser window |

Retention never follows the disk: personal data is not kept longer because there is room, and you can state a fixed duration. The number of recordings never goes below 300. When the volume fills (other files, larger backups), the next measurement lowers it back toward 300; below that, writes stop at 5 % free, as before. `GET /api/capacity` (admin), the MCP `capacity` tool and the startup log (`capacity from the disk`) show the value in force and why. If the volume's size cannot be read, 300 applies and the answer says so. The window is advertised to the server SDKs in each capture response (`x-openlens-stats-window`); 10 minutes applies until the route stats are counted, and an app can pin its own with `statsWindowSeconds`.

To give OpenLens less (a volume shared with other services) or more of the volume, set `OPENLENS_DISK_BUDGET` to a share (`40%`, 1 to 95 %) or a number of bytes (`500GB`, `2TiB`); it never goes past what leaves 5 % free.

## Security

The chart's defaults:
- a non-root user (uid 65532) and a read-only root filesystem (`/tmp` is an `emptyDir`);
- no Linux capabilities, seccomp `RuntimeDefault`, and no service-account token;
- probes on `/healthz`.

The capture endpoint must be public for browsers. Behind the Ingress it is rate-limited per client address and per project key (`429` with `Retry-After`, which the SDK honours); keep `X-Forwarded-For` set by the Ingress. The dashboard and the API are protected by the token, by sessions and by optional OIDC. If you put a WAF or an auth proxy in front, exempt `/capture`, `/flags`, `/r`, `/c.js`, `/r.js` and `/s.js` (and `/v1/metrics` if apps send OpenTelemetry metrics), or the SDK stops reporting.
