# Privacy

What OpenLens stores, for how long, where, and what it never stores. Everything stays on your server (and on the S3 copy, if you set one).

## Stored

- **Ask** (#244, off unless configured): the project's aggregates, feature map and goals go to the configured model provider, plus the questions asked. Event properties, URLs, replays and ids are never sent, unless an admin allows person-level detail for a project. Conversations are kept 30 days. See docs/ai.md.
- **Flag properties** (#279): `plan`, `country` and the `properties` passed to `identify` go to `/flags` in the query string to evaluate rules (at most 20, 100 characters each). They are not stored by OpenLens, but proxies may log query strings: do not target on personal data.
- **Events** sent by your app: name, user id (`distinct_id`), session id, time, page URL (without its query string and fragment, except the campaign tags `utm_*`, `ref` and `gclid`) and path, feature, role, account, plan, environment, release, active flags and the properties you send. Page views also get the **country** (looked up from the address, which is then dropped), **device class, browser and OS** (parsed from the user agent, which is then dropped), and the **referrer's host** (never its path or query).
- **Session recordings** (replays), with every input and every text masked, except the parts the app marks `data-openlens-unmask` (labels, menus); `data-openlens-mask` inside them stays hidden.
- **Clicks:** the element's tag, its feature, and up to 60 characters of its text, only when the element is under `data-feature` (the app's own label) or when the text holds neither an `@` nor a run of more than 6 digits.
- **People:** role, account, plan, first and last seen; role history.
- **Dashboard users:** email, name, role, time zone, last sign-in. Login links and sessions are signed; secrets (S3) are sealed.
- **Audit log** of admin actions (who, when, what), kept for the life of the install.

## On the visitor's device

- **By default**, the SDK keeps in the browser's `localStorage` (no cookie): an anonymous id, the session id and its last activity, what `identify` was given (user id, role, account, plan, and the properties flag rules target), the active flags and variants, and which surveys were answered. Some regulators treat `localStorage` like a cookie: ask for consent where that applies, and the SDK respects it.
- **With `data-cookieless`**, it stores nothing on the device (see *Cookieless mode* below).

## Never stored

- IP addresses (used for the country and the rate limit, then dropped).
- Raw user agents and referrer paths.
- Query strings, except the campaign tags `utm_*`, `ref` and `gclid`, kept in the page URL (see *Events* above).
- Input values in recordings.

## Retention (fixed, D-015)

- Events: 400 days. Recordings: 30 days after their last chunk. Uptime checks: 7 days.
- Backups and the S3 copy follow your own schedule and retention.

## Rights of the people you measure

- **Access:** a person's profile → *Export their data (JSON)* (admins), or `GET /p/{project}/people/{id}/export.json` with the admin token. Each export is in the audit log.
- **Erasure:** a person's profile → *Forget this person* (admins), or `DELETE /api/projects/{project}/people/{id}/forget`. Their events (and those of the anonymous ids linked to them by `identify`) are rewritten out of the files at once, their recordings and profile are deleted, and anything they send later is dropped. The list of forgotten ids stays in the metadata, so a restore from an older backup is cleaned again at the next compaction (within 10 minutes). The S3 copy and backups age out under your own retention.

## Audience measurement without consent

Some EU regulators exempt audience measurement from consent when it stores nothing on the device, keeps no person-level trail and records no session. With OpenLens, that is the cookieless mode below with replay off: `<script src=".../c.js" data-key="pk_…" data-cookieless data-replay="off">`. Whether it applies to you is for your DPO or lawyer to decide; this page is not legal advice.

The SDK also sends nothing when the browser sends Do Not Track or Global Privacy Control.

## Cookieless mode

Add `data-cookieless` to the script tag (or `init({ cookieless: true })`). The SDK then writes nothing to the device (no cookie, no localStorage; its state stays in memory), and the server turns each anonymous visitor into a hash of a salt that changes every UTC day, the project, the address and the user agent; the address is not stored, and yesterday's salt is overwritten, so a visitor can't be followed from one day to the next. A session continues while the visitor is active within 30 minutes. Logged-in users keep their own id, but the daily id is never linked to it. Session replay is off in this mode.

What it changes: anonymous visitors count as new every day (returning visitors, retention and churn rely on logged-in users), and behind no proxy (no `X-Forwarded-For`) every request counts as a new visitor.
