# Connecting an app to OpenLens (agent guide)

You are the agent that maintains a web app. Your goal is that the product manager's **first login shows a populated Focus board**, with no manual setup on their side. Draft: the commands arrive in M2 (`ROADMAP.md`).

1. **Project.** `openlens-cli init --server <url> --name <app>` creates the project and prints its capture key. Put the key and the server URL in the app's existing config mechanism (env file, settings module). Never hard-code the key anywhere else.
2. **SDK.** Pick one:
   - Server-rendered app: add `<script src="<url>/c.js" data-key="<key>" defer></script>` to the main layout.
   - SPA: `npm install @openlens/web`, then call `openlens.init({ host, key })` at startup.

   The environment is detected automatically. Pass `release` from the build (git SHA or version).
3. **Identify.** Where the logged-in user becomes known, call `openlens.identify(user.id, { role, account, plan })`. Find the real role field (for example `user.role`, `membership.kind`). If none exists, ask the operator; do not invent one.
4. **Feature map.** Create `openlens.features.yaml` (format: `docs/schema/features.example.yaml`):
   - `roles`: list every raw role value found in the code and map it to a canonical role.
   - `features`: list the main user-facing features from the routes and actions. Give each one its entry points and a **success event**: the moment the user got value, for example `entry_saved` or `invoice_sent`. Add `openlens.track("<success event>")` where that moment happens, if the app does not emit it already.
   - Add `data-feature="<key>"` to each feature's main controls.
   - Then run `openlens-cli features push` and `openlens-cli features lint`.
5. **Backfill.** Export from the app's database:
   - users with `id, role, account, plan, created_at, last_seen_at`
   - past success events, as one row per domain record (for example every saved entry, with `user_id` and `created_at`)

   Write them as JSONL and run `openlens-cli import`.
6. **Verify.** `openlens-cli verify` must pass: events arrive, roles resolve, and every feature entry point fires.
7. **Keep it current.** Add this rule to the app's own `AGENTS.md` (or `CLAUDE.md`):
   > When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (entry points, success event) in the same change and run `openlens-cli features lint`.
8. **Hand over.** `openlens-cli login-link` prints a one-time link. Send it to the PM, together with a short note on what was wired and anything you could not do.
