# @openlens/web

The OpenLens browser SDK. It is the one documented non-Rust part of OpenLens (`docs/decisions.md`, D-003): about **3.3 kB gzipped** against a budget of 10 kB, with no runtime dependencies and no cookies.

## Install

Script tag. The host is the script's own origin, so the key is the only attribute you need:

```html
<script src="https://lens.example.com/c.js" data-key="pk_..." data-release="v1.4.2" defer></script>
```

npm:

```js
import openlens from "@openlens/web";
openlens.init({ host: "https://lens.example.com", key: "pk_...", release: "v1.4.2" });
```

Then, once the user is known:

```js
openlens.identify(user.id, { role: user.role, account: user.orgId, plan: user.plan });
```

## What it captures on its own

- Page views, including SPA route changes. Clicks on interactive elements, with `$feature` taken from the nearest `data-feature` attribute and a `$rage` flag on 3 quick clicks. Form submits. **Input values are never captured.**
- Sessions: a new one after 30 minutes of inactivity or a UTC day change.
- Errors (`$error`): uncaught errors and unhandled rejections, at most 5 of the same message per page.
- Web Vitals (`$vital`): LCP, INP, CLS, FCP and TTFB, rated good / needs-improvement / poor.
- Failed requests (`$request_failed`): fetch/XHR 5xx and network errors, as path and status only.
- `$environment`: detected from the hostname (localhost → development, `staging.` → staging, preview hosts → preview, otherwise production).
- `$release`: from `init`, `data-release` or `<meta name="openlens-release">`.

## API

`init`, `identify(id, {role, account, plan})`, `track(event, props)`, `feature(key)`, `reset()` (on logout), `setConsent(bool)`, `flush()`.

Privacy: Do-Not-Track is respected, and `consent: "pending"` holds events until `setConsent(true)` is called. Add `data-openlens-ignore` to any element whose subtree should never be captured.

## Develop

```sh
npm ci && npm run build && npm run check   # check = typecheck + size budget
```

`dist/` is committed so that the Rust server can embed `openlens.min.js` without needing Node. CI fails if `dist/` is stale.
