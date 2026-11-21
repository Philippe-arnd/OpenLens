# Connecting an app to OpenLens (agent guide)

You are the agent that maintains a web app. Your goal is that the product manager's **first login shows a populated Focus board**, with no manual setup on their side.

**Available today:** `openlens-cli` (`init`, `features lint|push`, `verify`, `login-link`, `sql`, `guide`), the HTTP API below, the SDK and the dashboard. Steps marked *(M2)* (backfill, MCP) are not released yet: skip them and say so in your hand-over note.

`openlens-cli` reads `OPENLENS_SERVER` and `OPENLENS_TOKEN` (or `--server`/`--token`), prints JSON, and exits non-zero when something needs fixing. It is in the server image (`docker exec <container> openlens-cli …`), or `cargo install --git <repo> openlens-cli`.

API quick reference (send `Authorization: Bearer <admin token>` on every call):

| Call | Purpose |
| --- | --- |
| `POST /api/projects` `{"name": "My app"}` | Create a project. Returns `key`, a ready-made `snippet` and this guide. |
| `GET /api/projects` | List projects and their keys. |
| `POST /api/projects/<id>/sql` `{"sql": "SELECT …"}` | Read-only SQL on table `events`. Check that events arrive. |
| `GET /api/projects/<id>/board` | The Focus board numbers as JSON. |
| `POST /api/login-links` | A one-time login link for the PM, valid 7 days. |

1. **Project.** `openlens-cli init --name <app>` (or `POST /api/projects`) creates the project and returns its capture key and snippet. Put the key and the server URL in the app's existing config mechanism (env file, settings module). Never hard-code the key anywhere else.
2. **SDK.** Pick one:
   - Server-rendered app: add `<script src="<url>/c.js" data-key="<key>" defer></script>` to the main layout.
   - SPA: `npm install @openlens/web`, then call `openlens.init({ host, key })` at startup.

   The environment is detected automatically. Pass `release` from the build (git SHA or version).
3. **Identify.** Where the logged-in user becomes known, call `openlens.identify(user.id, { role, account, plan })`. Find the real role field (for example `user.role`, `membership.kind`). If none exists, ask the operator; do not invent one.
4. **Feature map.** Create `openlens.features.yaml` (format: `docs/schema/features.example.yaml`):
   - `roles`: list every raw role value found in the code and map it to a canonical role.
   - `features`: list the main user-facing features from the routes and actions. Give each one its entry points and a **success event**: the moment the user got value, for example `entry_saved` or `invoice_sent`. Add `openlens.track("<success event>")` where that moment happens, if the app does not emit it already.
   - Add `data-feature="<key>"` to each feature's main controls.
   - Run `openlens-cli features lint --src .` (offline: validates the map and checks that each feature's `data-feature`, route and success event exist in the code), then `openlens-cli features push --project <id>`. Commit the file: it is the source of truth.
5. **Backfill** *(M2)*. Export from the app's database:
   - users with `id, role, account, plan, created_at, last_seen_at`
   - past success events, as one row per domain record (for example every saved entry, with `user_id` and `created_at`)

   Write them as JSONL and run `openlens-cli import`.
6. **Verify.** Load a page of the app and click through the main features, then run `openlens-cli verify --project <id>`. It must say `"ok": true`: events arrive, users are identified with a role, a release is tagged, and the map is pushed. It also lists each feature's events, so a feature at 0 has not fired yet.
7. **Keep it current.** Add this rule to the app's own `AGENTS.md` (or `CLAUDE.md`):
   > When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (entry points, success event) in the same change and run `openlens-cli features lint`.
8. **Hand over.** `openlens-cli login-link` returns a one-time link. Send it to the PM, together with a short note on what was wired and anything you could not do.
