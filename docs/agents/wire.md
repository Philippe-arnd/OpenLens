# Connecting an app to OpenLens (agent guide)

You are the agent that maintains a web app. Your goal is that the product manager's **first login shows a populated Focus board**, with no manual setup on their side.

**Available today:** the HTTP API (admin bearer token), the SDK, and the dashboard. Steps marked *(M2)* need `openlens-cli`, which is not released yet. Skip them and say so in your hand-over note.

API quick reference (send `Authorization: Bearer <admin token>` on every call):

| Call | Purpose |
| --- | --- |
| `POST /api/projects` `{"name": "My app"}` | Create a project. Returns `key`, a ready-made `snippet` and this guide. |
| `GET /api/projects` | List projects and their keys. |
| `POST /api/projects/<id>/sql` `{"sql": "SELECT …"}` | Read-only SQL on table `events`. Check that events arrive. |
| `GET /api/projects/<id>/board` | The Focus board numbers as JSON. |
| `POST /api/login-links` | A one-time login link for the PM, valid 7 days. |

1. **Project.** `POST /api/projects` (later `openlens-cli init`) creates the project and returns its capture key. Put the key and the server URL in the app's existing config mechanism (env file, settings module). Never hard-code the key anywhere else.
2. **SDK.** Pick one:
   - Server-rendered app: add `<script src="<url>/c.js" data-key="<key>" defer></script>` to the main layout.
   - SPA: `npm install @openlens/web`, then call `openlens.init({ host, key })` at startup.

   The environment is detected automatically. Pass `release` from the build (git SHA or version).
3. **Identify.** Where the logged-in user becomes known, call `openlens.identify(user.id, { role, account, plan })`. Find the real role field (for example `user.role`, `membership.kind`). If none exists, ask the operator; do not invent one.
4. **Feature map.** Create `openlens.features.yaml` (format: `docs/schema/features.example.yaml`):
   - `roles`: list every raw role value found in the code and map it to a canonical role.
   - `features`: list the main user-facing features from the routes and actions. Give each one its entry points and a **success event**: the moment the user got value, for example `entry_saved` or `invoice_sent`. Add `openlens.track("<success event>")` where that moment happens, if the app does not emit it already.
   - Add `data-feature="<key>"` to each feature's main controls.
   - *(M2)* Then run `openlens-cli features push` and `openlens-cli features lint`. Until then, commit the file anyway: it is the source of truth.
5. **Backfill** *(M2)*. Export from the app's database:
   - users with `id, role, account, plan, created_at, last_seen_at`
   - past success events, as one row per domain record (for example every saved entry, with `user_id` and `created_at`)

   Write them as JSONL and run `openlens-cli import`.
6. **Verify.** Load a page of the app, then run `SELECT event, role, count(*) FROM events GROUP BY 1, 2` through the SQL API. Your page views and identify must be there, with the role. *(M2: `openlens-cli verify` automates this.)*
7. **Keep it current.** Add this rule to the app's own `AGENTS.md` (or `CLAUDE.md`):
   > When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (entry points, success event) in the same change and run `openlens-cli features lint`.
8. **Hand over.** `POST /api/login-links` returns a one-time link. Send it to the PM, together with a short note on what was wired and anything you could not do.
