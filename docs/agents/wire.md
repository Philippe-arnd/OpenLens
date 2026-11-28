# Connecting an app to OpenLens (agent guide)

You are the agent that maintains a web app. Your goal is that the product manager's **first login shows a populated Focus board**, with no manual setup on their side.

**No OpenLens server yet?** Deploy one first with the [self-hosting guide](../deploy/README.md): [Docker](../deploy/docker.md), [binary + systemd](../deploy/systemd.md), [Kubernetes](../deploy/kubernetes.md) (Helm or Kustomize) or [Coolify](../deploy/coolify.md). None needs any configuration.

**Available today:** `openlens-cli` (`init`, `features lint|push`, `import`, `verify`, `sourcemaps`, `release`, `flags`, `login-link`, `sql`, `guide`, `mcp`), the HTTP API below, the SDK and the dashboard. Everything below is available. Two small reference apps wired this way live in `examples/` (`notes-ssr`: script tag; `tasks-spa`: npm package). Journal, a real app wired outside this repo, is the third check of the M2 exit.

**MCP:** `openlens-cli mcp --automation-read-root <app repo> --automation-write-root <scratch dir>` speaks MCP over stdio. Its tools are `guide`, `projects`, `init`, `feature_map_lint`, `feature_map_push`, `map_health`, `map_overview`, `feature_context`, `suggest_map_changes`, `verify`, `insights`, `kpi_report`, `board`, `errors`, `sql`, `flags`, `flag_set`, `login_link` and `record_release`, and it reads files only inside the read root.
**MCP:** `openlens-cli mcp --automation-read-root <app repo> --automation-write-root <scratch dir>` speaks MCP over stdio. Its tools include `guide`, `projects`, `init`, `feature_map_lint`, `feature_map_push`, `map_health`, `map_overview`, `feature_context`, `suggest_map_changes`, `suggested_features`, `set_feature`, `verify`, `insights`, `kpi_report`, `board`, `errors`, `sql`, `flags`, `flag_set`, `stale_flags`, `releases`, `deploy_hook` and `login_link` (the `tools/list` call gives them all), and it reads files only inside the read root.

`openlens-cli` reads `OPENLENS_SERVER` and `OPENLENS_TOKEN` (or `--server`/`--token`), prints JSON, and exits non-zero when something needs fixing. Prefer a scoped token to the admin token: an admin makes one with `openlens-cli tokens create --scope write --project <id> --label <agent>` (`read` is enough for `openlens-cli mcp`); it is printed once, and `tokens revoke <id>` ends it. It is in the server image (`docker exec <container> openlens-cli …`), or `cargo install --git <repo> openlens-cli`.

API quick reference (send `Authorization: Bearer <admin token>` on every call). Every route also answers under the versioned prefix `/api/v1/...`, which is the contract; `/api/...` stays as an alias. The full list is the OpenAPI document at `/api/openapi.json`:

| Call | Purpose |
| --- | --- |
| `POST /api/projects` `{"name": "My app"}` | Create a project. Returns `key`, a ready-made `snippet` and this guide. |
| `GET /api/projects` | List projects and their keys. |
| `POST /api/projects/<id>/sql` `{"sql": "SELECT …"}` | Read-only SQL on table `events`. Check that events arrive. |
| `GET /api/projects/<id>/board` | The Focus board numbers as JSON. |
| `POST /api/login-links` | A one-time login link for the PM, valid 24 hours (an invite with `email`: 7 days). Opening it shows a "Continue" button; only that POST uses the link, so link previews do not burn it. |

Errors are JSON: `{"code": "...", "message": "..."}`. Act on `code`, which is one of `invalid_request`, `unauthorized`, `forbidden`, `not_found`, `conflict`, `too_large`, `rate_limited`, `busy` (another long query is running: retry after `Retry-After`), `unavailable` and `internal`. Messages may change between releases. The old `error` field repeats the message for one more release.

1. **Project.** `openlens-cli init --name <app> --timezone <IANA zone>` (or `POST /api/projects`) creates the project and returns its capture key and snippet. `openlens-cli init` also makes the project's deploy hook URL and lists, under `keep_it_running`, the steps of [Keep it running](#keep-it-running) below: the hook URL is shown only there, so store it as a secret at once. Days and weeks of every KPI start in that zone: use the team's, from the app's config (a `TZ` or locale setting) or the commit history, `UTC` if unknown. `openlens-cli timezone <project> <zone>` changes it later. Put the key and the server URL in the app's existing config mechanism (env file, settings module). Never hard-code the key anywhere else.
2. **SDK.** Pick one:
   - Server-rendered app: add `<script src="<url>/c.js" data-key="<key>" defer></script>` to the main layout.
   - SPA: `npm install @openlens-obs/web`, then call `openlens.init({ host, key })` at startup.

   The environment is detected automatically from the host. A staging or preview host whose name does not say so is stored as `unknown`: declare it with `openlens-cli env rule <host> staging` (MCP `environments` with `host` and `env`), and `ignore` the developers' own hosts. Events stored before the rule keep their old environment: count what would move with `env relabel <host> --dry-run` (MCP `environments` with `relabel`, a dry run by default), then run it without `--dry-run` (MCP `dry_run: false`). If the app sends its own environment names, map them with `env alias <name> <environment>` (MCP `alias` and `to`). Only if the operator asks to refuse events from other sites, preview an allow-list with `env allow <host…>` (MCP `environments` with `allow`), show them the hosts it would drop, and save it with `--confirm` (MCP `confirm: true`). If staging has its own address, check it with `env monitor staging <url>` (MCP `monitor` and `url`; `level: notify` only if the operator wants its alerts sent). A landing page on its own host (`www.example.com` next to `app.example.com`) gets a rule too, with the app's environment: the SDK then carries the anonymous visitor from one host to the other on a link click (`_ol=<id>` in that link only, removed from the address bar on arrival; never in cookieless mode or without consent), and a referrer from either host counts as `internal`, not `referral` (D-102). `GET /flags` lists those hosts as `sister_hosts`. Pass `release` from the build (git SHA or version). For a framework, follow its recipe in [Framework recipes](#framework-recipes) below: init runs **in the browser only**, once.
3. **Identify.** Where the logged-in user becomes known, call `openlens.identify(user.id, { role, account, plan, created_at })`: right after account creation (sign-up), not only at login, so the visit that led to the sign-up belongs to the new account. Find the real role field (for example `user.role`, `membership.kind`). If none exists, ask the operator; do not invent one. Pass the account's creation date as `created_at` (for example `user.created_at`, RFC 3339 or a `Date`) when the app has one: it is the person's signup, so a customer who logs in for the first time after the wiring is not counted as a new account (an internal tool passes the date the colleague joined it). Mark the team's own accounts (staff, testers, the founders) with `internal: true`, for example from the email domain or an `is_staff` flag: the board leaves them out by default, so their clicks don't weigh on a small app.
3b. **Privacy.** Session replay is on and masks every input and every text; add `data-openlens-unmask` to the app's chrome (navigation, menus, buttons) so replays stay readable. Also add `data-openlens-mask` to elements that *display* personal or secret data (emails, addresses, balances, tokens) and `data-openlens-block` to anything that must never be recorded (payment iframes, documents). If the operator wants no replay, use `replay: false`.
4. **Feature map.** Create `openlens.features.yaml` (format: `docs/schema/features.example.yaml`):
   - `roles`: list every raw role value found in the code and map it to a canonical role.
   - `features`: list the main user-facing features from the routes and actions. Give each one its entry points and a **success event**: the moment the user got value, for example `entry_saved` or `invoice_sent`. Add `openlens.track("<success event>")` where that moment happens, if the app does not emit it already.
   - Add `data-feature="<key>"` to each feature's main controls.
   - Run `openlens-cli features lint --src .` (offline: validates the map and checks that each feature's `data-feature`, route and success event exist in the code), then `openlens-cli features push --project <id> -m "<why>"`. Commit the file: it is the source of truth. See [Cutting features well](#cutting-features-well) for how big a feature should be.
   - Once real usage arrives, `openlens-cli features lint --live --project <id>` measures the pushed map (see below); add `--min-score 70` to make CI fail when it degrades.
5. **Backfill.** So the first login shows history, not an empty week, export from the app's database to one JSONL file:
   - **one line per user:**
     `{"type":"person","id":"42","role":"owner","account":"acme","plan":"pro","created_at":"2026-03-02T10:00:00Z","last_seen_at":"2026-11-20T08:12:00Z"}`
   - **one line per past success**, i.e. a domain row that proves the user got value (an entry saved, an invoice sent):
     `{"event":"entry_saved","distinct_id":"42","timestamp":"2026-10-02T09:00:00Z"}`

   Use the same event names as the feature map's `success` fields. Then run `openlens-cli import --project <id> --file backfill.jsonl`. Re-running it is safe, because chunks that were already imported are skipped. **History from another analytics tool:** export its raw events (one row per event, CSV or JSON lines) and map its columns: `openlens-cli import --project <id> --file export.csv --format csv --map event=<event column>,distinct_id=<user column>,timestamp=<time column>,url=<page URL column>,referrer=<referrer column>`. Other columns become properties; times may be ISO 8601 or Unix seconds or milliseconds; imported events are marked `$backfill`. Export only ids, roles, plans and timestamps: no names, emails or content.
5b. **Source maps (minified builds).** If the build emits `*.map` files, add `openlens-cli sourcemaps --release <same release as the SDK> <build dir>` to the deploy step, after the build. Errors then show the original file and line. Do not serve the maps publicly for this: the CLI uploads them with the admin token.
5c. **Deploy hook.** Call the MCP tool `deploy_hook` (or `POST /api/projects/<id>/deploy-hook`) once: it returns a URL `…/hooks/deploy/<secret>`, shown only then. Store it as a secret of the platform or the CI, never in the repository, and paste it where the platform sends deploy notifications: Coolify (notifications → webhook), Vercel (webhooks, `deployment.*` events), Netlify (deploy notifications → outgoing webhook, started, succeeded and failed), Render (webhooks, deploy events; add `?env=<name>`), GitHub (a repository webhook with the *Deployment statuses* event), Flux (a `generic` Provider; an Alert's `eventMetadata` may add `environment` and `release`), Argo CD (a `webhook` service with the body `{"phase": "{{.app.status.operationState.phase}}", "revision": "{{.app.status.sync.revision}}", "at": "{{.app.status.operationState.finishedAt}}", "environment": "production"}`). Fly.io sends no deploy notification: after `fly deploy`, post `{"FLY_IMAGE_REF": "<image>", "commit": "<sha>"}`. Anything else: post `{"status": "started|succeeded|failed|rolled_back", "release": "…", "commit": "…", "environment": "…", "at": "<RFC 3339>"}` from the deploy step. The Releases page then shows each deploy's time, and failed deploys, before the first user arrives. A deploy is tied to a release by its name, or by its commit when the release name is that commit (as the Node SDK names it) or a prefix of it.
5d. **Record each release from the deploy step.** After the deploy succeeds, run `openlens-cli release <release> --commit <sha> [--sourcemaps <build dir>] [--env <environment>] [--wait]`, with the same release name the SDK sends. It records the release with its commit range: from `--from <sha>` when given, else from the commit of the environment's previous succeeded deploy (recorded this way or by the deploy hook), to `--commit`. It shows on the Releases page as a succeeded deploy, with its deploy time, before the first user arrives. Run it in the app's checkout with its full history: it reads when the range's first commit was made from git, which gives the release's lead time (a shallow clone leaves lead time unknown, and the output's `lead_time_note` says why). `--sourcemaps` uploads every `*.map` file under the directory for this release (this replaces step 5b). `--wait` then polls the release's verdict for up to 30 minutes and exits non-zero when it is *regressed*, or when no verdict came in time (a verdict needs enough users of the release, or 24 hours). The token needs the `write` scope. In GitHub Actions, use the action instead (it downloads the CLI, checks its checksum and runs it; Linux runners):

   ```yaml
   - uses: actions/checkout@v4
     with:
       fetch-depth: 0               # the history, for the lead time
   # … build and deploy …
   - uses: Philippe-arnd/OpenLens/actions/openlens-release@<OpenLens version tag>
     with:
       server: ${{ vars.OPENLENS_SERVER }}
       token: ${{ secrets.OPENLENS_TOKEN }}
       project: <id>
       release: ${{ github.sha }}   # the release name the SDK sends
       sourcemaps: dist             # optional
       wait: false                  # true: fail the job on a regressed verdict
   ```

   The action records `${{ github.sha }}` as the commit. Without CI, the MCP tool `record_release` (or `POST /api/projects/<id>/releases` with `{"release", "commit", "from", "environment", "first_commit_at"}`) records the same, without the source maps; `first_commit_at` is the author time (Unix seconds) of the oldest commit of the range, for the lead time.
6. **Verify.** Load a page of the app and click through the main features, then run `openlens-cli verify --project <id>`. It must say `"ok": true`: events arrive, users are identified with a role, a release is tagged, and the map is pushed. It also lists each feature's events, so a feature at 0 has not fired yet. Its check `visitor linked to the person` sends a test anonymous page view, then an `$identify` linking that id (a test person marked internal, in `development`, so it counts in no production number), and waits up to 45 s for the page view to belong to the person; its `detail` also says when the project has a single production host (no landing page to link). It does not change `ok`.
7. **Keep it current.** Add the block printed by `openlens-cli rules` to the app's own `AGENTS.md` (or `CLAUDE.md`). In short:
   > When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (entry points, success event) in the same change and run `openlens-cli features lint`. Run `openlens-cli release` in the deploy step, read `release_report` after each deploy and act on a `regressed` verdict, and remove the flags `stale_flags` reports ready to remove.

   Then follow [Keep it running](#keep-it-running).
8. **Flags (when shipping a change).** Put risky or experimental changes behind `openlens.isEnabled("<key>")`. Create the flag with `openlens-cli flags set <key> --rollout 10 [--roles admin]`, and widen it once the board shows its cohort is fine. Every event carries the user's active flags in `$flags`. To target by plan, country or any other property, add rules (all must match): `--rule 'plan in pro,team' --rule 'country not in FR'`. The browser SDK sends `plan` and `identify(id, { properties: { seats: 12 } })` to `/flags`; server SDKs take the same `plan` and `properties`; `country` comes from the address when the app does not send it. A user who did not send a property a rule tests does not get the flag.
8b. **Ask (optional).** To learn *why*, add a micro-survey right after a success event: `survey_set { key, question, kind: rating|yes_no|choice|text, trigger: "<success event>", sample: 10 }`. Keep it to one short question about the moment that just happened. Results are on the Surveys page and in `surveys`.
9. **Hand over.** `openlens-cli login-link` returns a one-time link. Send it to the PM, together with a short note on what was wired and anything you could not do. To invite other people with limited access, use `openlens-cli login-link --email <email> --role viewer|admin [--projects <id,id>]`. Viewers can look but not change anything; admins also manage flags and the team (`/users`).

## Landing page and app

A marketing site on one host (`www.example.com`) and the app on another (`app.example.com`) are one product: wire them as one project, so a visitor who reads the landing page and then signs up is one person, with the landing's channel.

- **The same project key** on both: the same snippet (or `init({ host, key })`) on the landing page and in the app. Not two projects.
- **Host rules listing both hosts**, with the same environment: `openlens-cli env rule www.example.com production` and `openlens-cli env rule app.example.com production` (MCP `environments` with `host` and `env`). The SDK then carries the anonymous id from one host to the other on a link click, and a referrer from either host counts as `internal` (D-102). A `*.example.com` rule classifies hosts but does not link them: name each host.
- **`identify` right after account creation**, not only at login, with `created_at` (step 3). The visit that led to the sign-up is then given to the new account.
- `openlens-cli verify` checks the link (`visitor linked to the person`) and says when the project has only one production host.

An internal tool, or an app without a public landing page, has nothing to do here.

## Keep it running

Once the app is wired, every release should be measured without anyone asking. `openlens-cli init` prints these steps under `keep_it_running`:

- **Deploy hook.** `init` makes the URL (the MCP tool `deploy_hook` makes a new one later, and the old one then stops working). Store it as a secret of the platform or the CI and paste it where the platform sends deploy notifications (step 5c).
- **Release step.** Add `openlens-cli release` (or the `openlens-release` GitHub Action) to the deploy step, after the build, with the release name the SDK sends.
- **Rules.** Add the block printed by `openlens-cli rules` to the app's `AGENTS.md`. It tells the app's agents to keep the feature map and its code paths (routes, `data-feature` controls, success events) current, to keep the release step, to read `release_report` through MCP after each deploy and act on a `regressed` verdict, and to remove the flags `stale_flags` reports in `ready_to_remove`.
- **Status announcements.** Around a risky deploy, post what users should know and resolve it after: `openlens-cli announce post "Deploying v2.3" --level degraded`, then `openlens-cli announce close <id>` (MCP `announce`, `announcement_update`, `announcement_close`; a write token). The text is plain; markup is shown as text.
- **Check.** `openlens-cli verify --project <id>` (MCP `verify`) lists the release layers under `release_layers`, each `active` or not, with its `detail` and a `fix`:

| Layer | Active when |
| --- | --- |
| build fingerprint seen | an event of the last 7 days carries the browser SDK's `$build` fingerprint |
| deploy hook configured | the project has a deploy hook URL |
| releases recorded | a deploy was recorded in the last 90 days (by the hook or the release step) |
| verdicts present | a release of the last 90 days has a verdict (`detail` says when the server does not judge releases yet) |

The release layers never make `verify` fail: `ok` stays about the wiring.

**Feature map from production.** `suggested_features` turns what production uses and no feature claims (map health's `coverage`, `drift` and `undeclared` findings, 30 days, production only) into proposed entries. Check each against the code, add its success event, then `set_feature { feature }` adds or replaces that entry (by key) and pushes the map as a new version. It returns the whole map as YAML: write it to `openlens.features.yaml` and commit it, since the file stays the source of truth and pushing an older file would undo the change. The stored map's comments are not kept.

**Flags ready to remove.** `stale_flags` reads the flags and the production page views of the last 90 days. A flag is ready to remove when it is **off** and was seen before but on no page view of the last 14 days (delete it and the code behind `isEnabled`), or **on for everyone** (rollout 100, no roles, accounts, rules, users, variants or holdout) and carried by 95% or more of at least 100 production page views of the last 14 days (delete it and keep the code path users get now; the first visit of a user is sent before the SDK knows its flags, hence 95%). Off flags never seen in 90 days are listed under `never_seen`, and flags still rolling out or targeted under `in_rollout`.

## Cutting features well

A feature is **one job a user comes to do**, small enough that its success event means the job is done, big enough that a PM would put it on a roadmap.

- **Good:** `write-entry` (routes `/entries/new`, `/entries/:id/edit`, success `entry_saved`), `invite-teammate` (success `invite_sent`), `export-pdf` (success `pdf_downloaded`).
- **Too big:** `editor` covering writing, sharing and exporting: its success rate mixes three jobs and moves for no clear reason. Split it, one success event each.
- **Too small:** `click-bold-button`: a step, not a job. List it in the parent's `steps` instead.
- **Not a feature:** `/settings/*` as a catch-all, the home page, the login page. Leave plumbing pages out, or group them in one `account` feature without a success event.
- **Routes must not overlap**: `/docs/*` also matches `/docs/:id/edit`, and the first feature in the file wins. Make them disjoint, or put the narrower one first.

Optional fields:

| Field | Use it for |
| --- | --- |
| `area: Writing` | Group features (one level only). The map page and `map_overview` roll uses and users up by area. |
| `requires: [write-entry]` | Features a user normally uses first. The **adoption ladder** shows how many go from one to the next, and the health check flags a requirement most users skip. Unknown keys and loops are refused. |
| `renamed_from: [editor]` | Keep history across a rename: events still tagged with the old key count for the new one. A former key cannot still be a feature. |
| `server: ["POST /api/entries"]` | The feature's key actions on the server, `METHOD /route` as the server SDK reports it. A regression of one is named in the release verdict, and its alert is urgent. |
| `paths: ["src/export/**"]` | The feature's code, as globs from the repository root: `*` (inside one folder), `**` (any depth), `?` (one character), at most 20 per feature, 200 characters each, no leading `/` and no `..`. A release's changed files are matched against them to say which features it touched; files matching no feature point at gaps in the map. Keep them current when code moves. |

Older servers refuse maps with these fields (the format is strict): upgrade the server before pushing them.

**Map health.** After a week of real traffic, run `openlens-cli features lint --live` (MCP `map_health`). It scores the pushed map out of 100 and lists findings ranked by impact, each with the numbers and a YAML change: pages and clicks no feature claims (with a suggested route and key), overlapping routes, dead features, success events that never fire or fire for every user (usually on entry, not on value), `data-feature` values the map does not declare, and `requires` usage contradicts. `suggest_map_changes` returns only the changes; nothing is applied until you edit the file and push it.

## Framework recipes

Three rules hold in every framework:
- **Init in the browser, once.** Importing `@openlens-obs/web` on the server is harmless (`init` does nothing without `window`), but call `init` from client-only code.
- **Release at build time.** Inject the commit or version into a public build variable: for example `GITHUB_SHA`, `VERCEL_GIT_COMMIT_SHA`, `CF_PAGES_COMMIT_SHA`, Coolify's `SOURCE_COMMIT`, or `git rev-parse --short HEAD`. Use the same value for `openlens-cli sourcemaps --release`. Without one, the server still tells builds apart by their script and stylesheet names and calls each new one `YYYY-MM-DD.N`; the Node SDK reads the host's commit variable by itself.
- **Identify on every load where the user is known, and reset on logout.** `reset()` while nobody is identified does nothing, so it is safe to call whenever there is no session.

Route changes are picked up on their own (History API and `#/` routes), so no router hook is needed.

### Vite SPA (React, Vue, Svelte, Solid) and Angular

```ts
// src/main.ts, before mounting the app
import openlens from "@openlens-obs/web";
openlens.init({
  host: import.meta.env.VITE_OPENLENS_HOST,
  key: import.meta.env.VITE_OPENLENS_KEY,
  release: import.meta.env.VITE_RELEASE, // VITE_RELEASE=$(git rev-parse --short HEAD) vite build
});
```

Angular: do the same in `main.ts`, before `bootstrapApplication`, with the values in `environment.ts`.

### Next.js (App Router)

```tsx
// app/openlens.tsx
"use client";
import { useEffect } from "react";
import openlens from "@openlens-obs/web";

export function OpenLens({ user }: { user?: { id: string; role: string } }) {
  useEffect(() => {
    openlens.init({
      host: process.env.NEXT_PUBLIC_OPENLENS_HOST!,
      key: process.env.NEXT_PUBLIC_OPENLENS_KEY!,
      release: process.env.NEXT_PUBLIC_RELEASE,
    });
  }, []);
  useEffect(() => {
    if (user) openlens.identify(user.id, { role: user.role });
    else openlens.reset();
  }, [user?.id]);
  return null;
}
```

```tsx
// app/layout.tsx: render it once, with the session user from your auth library
<body>
  <OpenLens user={session?.user} />
  {children}
</body>
```

```js
// next.config.js: the release at build time
module.exports = { env: { NEXT_PUBLIC_RELEASE: process.env.VERCEL_GIT_COMMIT_SHA ?? process.env.SOURCE_COMMIT ?? "dev" } };
```

Pages Router: do the same in `pages/_app.tsx`. With the script tag, use `<Script src="…/c.js" data-key="pk_…" data-release={release} strategy="afterInteractive" />` from `next/script`.

### Nuxt 3

```ts
// plugins/openlens.client.ts (the .client suffix keeps it out of the server)
import openlens from "@openlens-obs/web";
export default defineNuxtPlugin(() => {
  const c = useRuntimeConfig().public;
  openlens.init({ host: c.openlensHost, key: c.openlensKey, release: c.release });
  return { provide: { openlens } };
});
```

```ts
// nuxt.config.ts: set at build or runtime with NUXT_PUBLIC_OPENLENS_HOST, NUXT_PUBLIC_OPENLENS_KEY, NUXT_PUBLIC_RELEASE
export default defineNuxtConfig({ runtimeConfig: { public: { openlensHost: "", openlensKey: "", release: "" } } });
```

Identify where the session is known, for example in a `watch` on the user in `app.vue`: `useNuxtApp().$openlens.identify(...)`.

### SvelteKit

```ts
// src/hooks.client.ts: runs once, in the browser
import openlens from "@openlens-obs/web";
import { PUBLIC_OPENLENS_HOST, PUBLIC_OPENLENS_KEY } from "$env/static/public";
import { version } from "$app/environment"; // kit.version.name, e.g. the git SHA
openlens.init({ host: PUBLIC_OPENLENS_HOST, key: PUBLIC_OPENLENS_KEY, release: version });
```

Set `kit.version.name` to the commit in `svelte.config.js` (`process.env.GITHUB_SHA ?? …`). Identify in the root `+layout.svelte`, from the user returned by the layout's `load`:

```svelte
<script>
  import openlens from "@openlens-obs/web";
  export let data;
  $: if (data.user) openlens.identify(data.user.id, { role: data.user.role }); else openlens.reset();
</script>
```

Use `$effect` with Svelte 5.

### Astro

Astro pages are server-rendered, so use the script tag in the base layout. `is:inline` keeps Astro from bundling it:

```astro
---
// src/layouts/Base.astro
const release = import.meta.env.PUBLIC_RELEASE;
---
<script is:inline src="https://lens.example.com/c.js" data-key="pk_…" data-release={release} defer></script>
```

View transitions (`<ClientRouter />`) navigate with the History API, so page views are counted. For logged-in pages, identify in an inline script after the tag, the same way as for server-rendered apps below (`define:vars={{ user }}` passes the user from the frontmatter).

### Remix and React Router 7

```tsx
// app/entry.client.tsx, before hydrateRoot
import openlens from "@openlens-obs/web";
openlens.init({
  host: import.meta.env.VITE_OPENLENS_HOST,
  key: import.meta.env.VITE_OPENLENS_KEY,
  release: import.meta.env.VITE_RELEASE,
});
```

Identify in `app/root.tsx`, from the root loader's user: `useEffect(() => { user ? openlens.identify(user.id, { role: user.role }) : openlens.reset(); }, [user?.id])`.

### Server-rendered apps (Rails, Django, Laravel, Phoenix, Go…)

Use the script tag in the main layout, with `data-release` from the deploy (an env var read by the template). Identify in the same layout when a user is logged in:

```html
<!-- after the c.js tag; deferred scripts run before DOMContentLoaded -->
<script>
  addEventListener("DOMContentLoaded", () =>
    window.openlens?.identify({{ user.id | json }}, { role: {{ user.role | json }} }));
</script>
```

Write the values as JSON with the template engine (`|tojson` in Jinja, `json_script` in Django, `to_json` in Rails, `@json` in Blade), never as raw strings. Call `openlens.reset()` the same way on the page shown after logout. For events that happen on the server (a payment confirmed, a job finished), use a server SDK (Node, Python, Go, Ruby, PHP), or a plain `POST /capture` ([sdk/SERVER.md](../../sdk/SERVER.md)).

## Strict Content Security Policy

If the app sends a `Content-Security-Policy` header (or a `<meta http-equiv>`), allow the OpenLens server, or the SDK is blocked silently: no events, and only a console warning in the browser.

| Directive | Why | Add |
| --- | --- | --- |
| `script-src` | `c.js` (script tag), and the recorder `/r.js` and survey widget `/s.js` that the SDK loads lazily from the same server | `https://lens.example.com` |
| `connect-src` | events (`/capture`), flags and surveys (`/flags`), replay chunks (`/r`), all sent with `fetch` or `sendBeacon` | `https://lens.example.com` |

Nothing else is needed:
- **No `unsafe-inline` or `unsafe-eval`.** The SDK injects no inline scripts and evaluates nothing.
- **No `img-src`, `worker-src` or `frame-src`.** It uses no images, workers or frames.
- **Survey styles.** The survey card styles itself with a constructable stylesheet inside a shadow root, which `style-src` does not block in current browsers.
- **npm package.** The SDK is bundled with your code, so `script-src` only needs the server for `/r.js` and `/s.js`. Without replay or surveys, `connect-src` alone is enough.

```http
Content-Security-Policy: default-src 'self'; script-src 'self' https://lens.example.com; connect-src 'self' https://lens.example.com
```

- **Nonces with `'strict-dynamic'`:** give the `c.js` tag the page's nonce. The scripts it loads are then trusted through `'strict-dynamic'`, and only `connect-src` needs the host.
- **Proxy through the app's own domain** (next section): `'self'` covers everything, so the CSP needs no change.
- **Check it:**
  1. Open a page with the browser console open: any `Refused to load` or `Refused to connect` line names the missing directive.
  2. Run `openlens-cli verify`.

How to set the header:
- **Next.js:** `headers()` in `next.config.js`.
- **SvelteKit:** `kit.csp` (add the host to `script-src` and `connect-src`).
- **Nuxt:** `nuxt-security`'s `contentSecurityPolicy`.
- **Rails:** `content_security_policy` with `policy.script_src :self, "https://lens.example.com"` and `policy.connect_src :self, "https://lens.example.com"`.
- **Django:** `django-csp`, with `script-src` and `connect-src` in `CONTENT_SECURITY_POLICY` (version 4) or `CSP_SCRIPT_SRC` and `CSP_CONNECT_SRC` (version 3).

## Ad blockers: proxy through the app's domain

Some content blockers drop requests to third-party analytics hosts. That undercounts users, and unevenly: blocker use differs by audience. To avoid it, serve OpenLens under a path of the app's own domain, then point the SDK at that path. CORS and CSP become same-origin as a bonus.

1. **Pick a neutral prefix**, for example `/_l`. Avoid words that block lists match, such as `analytics`, `track`, `collect` or `openlens`.
2. **Forward `/_l/*` to the OpenLens server with the prefix removed:** `/_l/capture` → `https://lens.example.com/capture`. The paths in use are `/c.js`, `/r.js`, `/s.js`, `/capture`, `/flags` and `/r`. Requests are at most 1 MB.
3. **Point the SDK at the prefix:**
   - **script tag:** `<script src="/_l/c.js" data-key="pk_…" defer></script>`. The server is wherever `c.js` came from, prefix included, so no other attribute is needed.
   - **npm:** `openlens.init({ host: "/_l", key: "pk_…" })`. A relative host works, as does `https://app.example.com/_l`.
4. **Check it:** in the browser's network tab, events go to `/_l/capture` on the app's domain. Then run `openlens-cli verify`.

The dashboard and the API stay on the OpenLens domain; only the SDK's traffic goes through the proxy.

**Nginx**

```nginx
location /_l/ {
    proxy_pass https://lens.example.com/;   # the trailing slash strips /_l/
    proxy_set_header Host lens.example.com;
    proxy_ssl_server_name on;
    client_max_body_size 1m;
}
```

**Caddy**

```caddy
handle_path /_l/* {
    reverse_proxy https://lens.example.com {
        header_up Host {upstream_hostport}
    }
}
```

**Traefik** (file provider; with Coolify, add it under the proxy's dynamic configuration)

```yaml
http:
  routers:
    app-lens:
      rule: Host(`app.example.com`) && PathPrefix(`/_l`)
      middlewares: [app-lens-strip]
      service: app-lens
  middlewares:
    app-lens-strip:
      stripPrefix:
        prefixes: [/_l]
  services:
    app-lens:
      loadBalancer:
        passHostHeader: false
        servers:
          - url: https://lens.example.com
```

**Next.js** (`next.config.js`)

```js
module.exports = {
  async rewrites() {
    return [{ source: "/_l/:path*", destination: "https://lens.example.com/:path*" }];
  },
};
```

**Vercel / Netlify** (static hosting)

```json
// vercel.json
{ "rewrites": [{ "source": "/_l/:path*", "destination": "https://lens.example.com/:path*" }] }
```

```text
# netlify: public/_redirects
/_l/*  https://lens.example.com/:splat  200
```

