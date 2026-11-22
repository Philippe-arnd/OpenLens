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

  If you don't, OpenLens generates a token, stores it in its volume and prints it once in its log (`generated admin token`).

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

- **Disk:** about **11 bytes per event** after compression, and events are kept 400 days. So 1 M events a day needs about 4.5 GB a year. Replays (kept 30 days) often weigh more than events: watch the PVC usage during the first week and size from it.
- **CPU:** about **80k events/s per vCPU** at ingestion. The default request (`100m`) is enough for most apps.
- **Memory:** the board keeps up to 4 M events of the 90-day window in memory, at about 160 bytes each (up to about 650 MB). The default limit of 1 GiB covers about 2 M events in 90 days. Raise it to 2 GiB above that.

## Security

The chart's defaults:
- a non-root user (uid 65532) and a read-only root filesystem (`/tmp` is an `emptyDir`);
- no Linux capabilities, seccomp `RuntimeDefault`, and no service-account token;
- probes on `/healthz`.

The capture endpoint must be public for browsers. Behind the Ingress it is rate-limited per client address and per project key (`429` with `Retry-After`, which the SDK honours); keep `X-Forwarded-For` set by the Ingress. The dashboard and the API are protected by the token, by sessions and by optional OIDC. If you put a WAF or an auth proxy in front, exempt `/capture`, `/flags`, `/r`, `/c.js`, `/r.js` and `/s.js`, or the SDK stops reporting.
