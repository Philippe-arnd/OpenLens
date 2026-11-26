# @openlens/web

The browser SDK of [OpenLens](https://github.com/Philippe-arnd/OpenLens): self-hosted product analytics by role, feature and release. **About 5.5 kB gzipped**, no runtime dependencies, no cookies.

It is the one non-Rust part of OpenLens (`docs/decisions.md`, D-003). It has to run inside your own pages.

## Install

**Script tag** (server-rendered apps). The server is wherever the script came from, path prefix included, so the key is the only attribute you need, also behind a proxy on your own domain:

```html
<script src="https://lens.example.com/c.js" data-key="pk_..." data-release="v1.4.2" defer></script>
```

**npm** (SPAs and frameworks):

```sh
npm install @openlens/web
```

```js
import openlens from "@openlens/web";
openlens.init({ host: "https://lens.example.com", key: "pk_...", release: import.meta.env.VITE_RELEASE });
```

Then, once the user is known (and on every page load where they are logged in):

```js
openlens.identify(user.id, { role: user.role, account: user.orgId, plan: user.plan });
```

On logout, call `openlens.reset()`.

Framework recipes (Next.js, Nuxt, SvelteKit, Astro, Remix), strict CSP and ad-blocker proxies are covered in the [wiring guide](https://github.com/Philippe-arnd/OpenLens/blob/main/docs/agents/wire.md). Importing the package on the server is safe: `init` does nothing without `window`.

## What it captures on its own

- **Page views**, including SPA route changes (History API and `#/` hash routes) and back/forward-cache restores. Prerendered pages count only once they are shown.
- **Clicks** on interactive elements, including inside web components (open shadow roots). `$feature` comes from the nearest `data-feature` attribute, inside or around the component, and 3 quick clicks set a `$rage` flag.
- **Form submits**, also inside web components. Input values are never captured.
- **Sessions:** a new one after 30 minutes of inactivity or a UTC day change. Tabs of the same app share the visitor and the session.
- **Errors** (`$error`): uncaught errors and unhandled rejections, at most 5 of the same message per page.
- **Web Vitals** (`$vital`): LCP, INP, CLS, FCP and TTFB, each rated good, needs-improvement or poor. Each also says where it comes from, without any text: the element (a short CSS selector, `$el`), LCP's element type, resource path and four parts (`$el_type`, `$res`, `$ttfb`, `$load_delay`, `$load_time`, `$render_delay`), INP's interaction type and three parts (`$input`, `$input_delay`, `$processing`, `$presentation`), and the device class (`$device`). Elements under `data-openlens-ignore` are not named; under `data-openlens-mask`, only their tag names are sent.
- **Failed requests** (`$request_failed`): fetch/XHR 5xx and network errors, sent as path and status only.
- **Environment** (`$environment`), detected from the hostname:
  - localhost → `development`;
  - `staging.` → `staging`;
  - preview hosts → `preview`;
  - anything else → `unknown`: the server's host rules then decide (Settings → Environments), and an unclassified host is never counted as production.
- **Release** (`$release`), from `init`, `data-release` or `<meta name="openlens-release">`.

**Delivery:** events are batched every 5 s and on page hide. A failed send (offline, 5xx, 429) is retried with the same event ids. Crawlers and monitoring robots are not captured.

## API

| Call | What it does |
| --- | --- |
| `init(config)` | Start. Config: `host`, `key`, `release?`, `environment?`, `autocapture?`, `consent?` (`"granted"`, `"pending"` or `"denied"`), `replay?` |
| `identify(id, { role, account, plan, internal })` | Name the user and the dimensions the board slices by. Links the anonymous visit before login. `internal: true` marks the team's own accounts, which the board leaves out by default. |
| `track(event, props?)` | A custom event, such as a feature's success moment |
| `feature(key, props?)` | Mark the use of a feature without a DOM element |
| `isEnabled(flagKey)` | Feature flag. Synchronous: the last answer is cached. |
| `reset()` | Forget the user (logout) |
| `setConsent(granted)` | Grant or withdraw consent at runtime |
| `flush()` | Send queued events now |

Flags are fetched at init and after `identify`. The active flags travel with every event as `$flags`.

## Privacy

- Do Not Track and Global Privacy Control are respected: nothing is sent.
- Page URLs are sent without their query string and fragment, except `utm_*`, `ref` and `gclid`.
- `consent: "pending"` holds events until `setConsent(true)` is called.
- `data-openlens-ignore`: nothing in that subtree is captured.
- **Session replay** is on by default. Once the page is idle and consent is granted, the recorder (`/r.js`, rrweb under MIT) loads from your OpenLens server.
- Not every session is recorded: the server sets a rate for about 300 recordings a day. A session left out keeps its last minute or two in memory and sends it only after an error or a rage click, then records the rest of the session.
- Every input value and every text is masked in replays; `data-openlens-unmask` shows the text of a subtree (menus, labels).
- `data-openlens-mask` masks text, both in replays and in the `$text`/`$href` of clicks. Outside `data-feature`, a click text with an `@` or more than 6 digits in a row is not sent.
- `data-openlens-block` subtrees are not recorded at all.
- To turn replay off, pass `replay: false` or add `data-replay="off"` to the script tag.

## Develop

```sh
npm ci && npm run build && npm run check   # check = typecheck + size budget
```

`dist/` is committed so that the Rust server can embed `openlens.min.js` without Node. CI fails if `dist/` is stale. Releases are published by `.github/workflows/npm.yml` on `v*` tags.

## License

AGPL-3.0-only, like the rest of OpenLens.
