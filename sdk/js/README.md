# @openlens/web

The browser SDK, and the one documented non-Rust part of OpenLens (`docs/decisions.md`, D-003). Not started (ROADMAP.md, M1).

Budget: **under 10 kB gzipped** for the core (capture, identify, features, flags, errors). Session recording loads lazily as a separate chunk.

Planned API:

```js
openlens.init({ host, key, autocapture: true, consent: "granted" | "pending" });
openlens.identify(id, { role, account, plan });
openlens.feature("export-pdf");           // mark a feature use explicitly
openlens.track("invoice_paid", { amount });
openlens.isEnabled("new-editor");         // feature switch
```
