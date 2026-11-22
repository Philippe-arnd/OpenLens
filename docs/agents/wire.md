# Connecting an app to OpenLens (agent guide)

You are the agent that maintains a web app. Your goal is that the product manager's **first login shows a populated Focus board**, with no manual setup on their side.

**Available today:** `openlens-cli` (`init`, `features lint|push`, `import`, `verify`, `sourcemaps`, `flags`, `login-link`, `sql`, `guide`, `mcp`), the HTTP API below, the SDK and the dashboard. Everything below is available. Two small reference apps wired this way live in `examples/` (`notes-ssr`: script tag; `tasks-spa`: npm package).

**MCP:** `openlens-cli mcp --automation-read-root <app repo> --automation-write-root <scratch dir>` speaks MCP over stdio. Its tools are `guide`, `projects`, `init`, `feature_map_lint`, `feature_map_push`, `verify`, `insights`, `kpi_report`, `board`, `errors`, `sql`, `flags`, `flag_set` and `login_link`, and it reads files only inside the read root.

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

   The environment is detected automatically. Pass `release` from the build (git SHA or version). For a framework, follow its recipe in [Framework recipes](#framework-recipes) below: init runs **in the browser only**, once.
3. **Identify.** Where the logged-in user becomes known, call `openlens.identify(user.id, { role, account, plan })`. Find the real role field (for example `user.role`, `membership.kind`). If none exists, ask the operator; do not invent one.
3b. **Privacy.** Session replay is on and masks every input. Also add `data-openlens-mask` to elements that *display* personal or secret data (emails, addresses, balances, tokens) and `data-openlens-block` to anything that must never be recorded (payment iframes, documents). If the operator wants no replay, use `replay: false`.
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
5b. **Source maps (minified builds).** If the build emits `*.map` files, add `openlens-cli sourcemaps --release <same release as the SDK> <build dir>` to the deploy step, after the build. Errors then show the original file and line. Do not serve the maps publicly for this: the CLI uploads them with the admin token.
6. **Verify.** Load a page of the app and click through the main features, then run `openlens-cli verify --project <id>`. It must say `"ok": true`: events arrive, users are identified with a role, a release is tagged, and the map is pushed. It also lists each feature's events, so a feature at 0 has not fired yet.
7. **Keep it current.** Add the rule printed by `openlens-cli rules` to the app's own `AGENTS.md` (or `CLAUDE.md`). In short:
   > When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (entry points, success event) in the same change and run `openlens-cli features lint`.
8. **Flags (when shipping a change).** Put risky or experimental changes behind `openlens.isEnabled("<key>")`. Create the flag with `openlens-cli flags set <key> --rollout 10 [--roles admin]`, and widen it once the board shows its cohort is fine. Every event carries the user's active flags in `$flags`.
8b. **Ask (optional).** To learn *why*, add a micro-survey right after a success event: `survey_set { key, question, kind: rating|yes_no|choice|text, trigger: "<success event>", sample: 10 }`. Keep it to one short question about the moment that just happened. Results are on the Surveys page and in `surveys`.
9. **Hand over.** `openlens-cli login-link` returns a one-time link. Send it to the PM, together with a short note on what was wired and anything you could not do. To invite other people with limited access, use `openlens-cli login-link --email <email> --role viewer|admin [--projects <id,id>]`. Viewers can look but not change anything; admins also manage flags and the team (`/users`).

## Framework recipes

Three rules hold in every framework:
- **Init in the browser, once.** Importing `@openlens/web` on the server is harmless (`init` does nothing without `window`), but call `init` from client-only code.
- **Release at build time.** Inject the commit or version into a public build variable: for example `GITHUB_SHA`, `VERCEL_GIT_COMMIT_SHA`, `CF_PAGES_COMMIT_SHA`, Coolify's `SOURCE_COMMIT`, or `git rev-parse --short HEAD`. Use the same value for `openlens-cli sourcemaps --release`.
- **Identify on every load where the user is known, and reset on logout.** `reset()` while nobody is identified does nothing, so it is safe to call whenever there is no session.

Route changes are picked up on their own (History API and `#/` routes), so no router hook is needed.

### Vite SPA (React, Vue, Svelte, Solid) and Angular

```ts
// src/main.ts, before mounting the app
import openlens from "@openlens/web";
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
import openlens from "@openlens/web";

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
import openlens from "@openlens/web";
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
import openlens from "@openlens/web";
import { PUBLIC_OPENLENS_HOST, PUBLIC_OPENLENS_KEY } from "$env/static/public";
import { version } from "$app/environment"; // kit.version.name, e.g. the git SHA
openlens.init({ host: PUBLIC_OPENLENS_HOST, key: PUBLIC_OPENLENS_KEY, release: version });
```

Set `kit.version.name` to the commit in `svelte.config.js` (`process.env.GITHUB_SHA ?? …`). Identify in the root `+layout.svelte`, from the user returned by the layout's `load`:

```svelte
<script>
  import openlens from "@openlens/web";
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
import openlens from "@openlens/web";
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

Write the values as JSON with the template engine (`|tojson` in Jinja, `json_script` in Django, `to_json` in Rails, `@json` in Blade), never as raw strings. Call `openlens.reset()` the same way on the page shown after logout. For events that happen on the server (a payment confirmed, a job finished), use the Node or Python server SDK, or a plain `POST /capture` ([sdk/SERVER.md](../../sdk/SERVER.md)).

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

