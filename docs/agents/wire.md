# Connecting an app to OpenLens (agent guide)

You are the agent that maintains a web app. Your goal is that the product manager's **first login shows a populated Focus board**, with no manual setup on their side.

**Available today:** `openlens-cli` (`init`, `features lint|push`, `import`, `verify`, `login-link`, `sql`, `guide`, `mcp`), the HTTP API below, the SDK and the dashboard. Everything below is available.

**MCP:** `openlens-cli mcp --automation-read-root <app repo> --automation-write-root <scratch dir>` speaks MCP over stdio. Its tools are `guide`, `projects`, `init`, `feature_map_lint`, `feature_map_push`, `verify`, `board`, `sql` and `login_link`, and it reads files only inside the read root.

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
5. **Backfill.** So the first login shows history, not an empty week, export from the app's database to one JSONL file:
   - **one line per user:**
     `{"type":"person","id":"42","role":"owner","account":"acme","plan":"pro","created_at":"2026-03-02T10:00:00Z","last_seen_at":"2026-11-20T08:12:00Z"}`
   - **one line per past success**, i.e. a domain row that proves the user got value (an entry saved, an invoice sent):
     `{"event":"entry_saved","distinct_id":"42","timestamp":"2026-10-02T09:00:00Z"}`

   Use the same event names as the feature map's `success` fields. Then run `openlens-cli import --project <id> --file backfill.jsonl`. Re-running it is safe, because chunks that were already imported are skipped. Export only ids, roles, plans and timestamps: no names, emails or content.
6. **Verify.** Load a page of the app and click through the main features, then run `openlens-cli verify --project <id>`. It must say `"ok": true`: events arrive, users are identified with a role, a release is tagged, and the map is pushed. It also lists each feature's events, so a feature at 0 has not fired yet.
7. **Keep it current.** Add this rule to the app's own `AGENTS.md` (or `CLAUDE.md`):
   > When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (entry points, success event) in the same change and run `openlens-cli features lint`.
8. **Hand over.** `openlens-cli login-link` returns a one-time link. Send it to the PM, together with a short note on what was wired and anything you could not do.
