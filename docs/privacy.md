# Privacy

What OpenLens stores, for how long, where, and what it never stores. Everything stays on your server (and on the S3 copy, if you set one).

## Stored

- **Ask** (#244, off unless configured): the project's aggregates, feature map and goals go to the configured model provider, plus the questions asked. Event properties, URLs, replays and ids are never sent, unless an admin allows person-level detail for a project. Conversations are kept 30 days. See docs/ai.md.
- **Flag properties** (#279): `plan`, `country` and the `properties` passed to `identify` go to `/flags` in the query string to evaluate rules (at most 20, 100 characters each). They are not stored by OpenLens, but proxies may log query strings: do not target on personal data.
- **Events** sent by your app: name, user id (`distinct_id`), session id, time, page URL (without its query string and fragment, except the campaign tags `utm_*`, `ref` and `gclid`) and path, feature, role, account, plan, environment, release, active flags and the properties you send. Page views also get the **country** (looked up from the address, which is then dropped), **device class, browser and OS** (parsed from the user agent, which is then dropped), and the **referrer's host** (never its path or query).
- **Session recordings** (replays), with every input and every text masked, except the parts the app marks `data-openlens-unmask` (labels, menus); `data-openlens-mask` inside them stays hidden. Not every session is recorded (#327, D-035): about 300 a day per project, drawn at random. A session left out still records its last one or two minutes into the browser's memory, and sends them only when an error or a rage click happens in it (it is then recorded to its end); otherwise that buffer never leaves the page, and withdrawing consent empties it.
- **Clicks:** the element's tag, its feature, and up to 60 characters of its text, only when the element is under `data-feature` (the app's own label) or when the text holds neither an `@` nor a run of more than 6 digits.
- **Web Vitals** (#467): each measurement's value and rating, the device class (`$device`: phone, tablet, desktop), and for the slow element a short CSS selector (`$el`: tags, an id and up to two hand-written classes, at most 120 characters, never any text), its type, and the path of the resource it loaded (`$res`, without query string or fragment). Nothing is sent for an element under `data-openlens-ignore`, and only tag names under `data-openlens-mask`.
- **Build fingerprint** (`$build`, #495): 8 hexadecimal characters, a hash of the addresses of the scripts and stylesheets the page loads from its own origin (path and query string, no other site). It says which build of the app ran, holds no personal data, and is the same for every visitor of that build. The server keeps, per project and environment, each fingerprint with the release name it gave it (at most 500).
- **People:** role, account, plan, first and last seen; role history.
- **Dashboard users:** email, name, role, time zone, last sign-in. Login links and sessions are signed; secrets (S3) are sealed.
- **Audit log** of admin actions (who, when, what), kept for the life of the install.

## On the visitor's device

- **By default**, the SDK keeps in the browser's `localStorage` (no cookie): an anonymous id, the session id and its last activity (and whether that session is recorded, and whether it sends the sampled events of D-107), what `identify` was given (user id, role, account, plan, and the properties flag rules target) with a hash of it and the day it was last sent, the active flags and variants, and which surveys were answered. When the page closes with more to send than the browser allows, the unsent requests (events and replay chunks) are kept there too, for at most 24 hours and 256 KB, and sent on the next visit (#433); withdrawing consent deletes them. Some regulators treat `localStorage` like a cookie: ask for consent where that applies, and the SDK respects it.
- **With `data-cookieless`**, it stores nothing on the device (see *Cookieless mode* below).

## Never stored

- IP addresses (used for the country and the rate limit, then dropped).
- Raw user agents and referrer paths.
- Query strings, except the campaign tags `utm_*`, `ref` and `gclid`, kept in the page URL (see *Events* above).
- Input values in recordings.

## Retention (fixed, D-015)

- Click labels and positions (`$text`, `$tag`, `$id`, `$href`, `$x`, `$y`, `$vw`): 30 days. After that, a click keeps only its page, time and rage flag, and the day's page rollups keep the counts per click-map cell and the 20 most clicked labels per page (D-028).
- Events: 400 days. Daily rollups (counts per 15 minutes; per person only their number of events and errors, and per person and day their events, sessions, errors and completed tasks for each environment, release, role and feature they used, with their first and last event time; per session and day its random id, person, environment, release and role, first and last event time and its number of interactions, errors and completed tasks; per page its path, click-map counts, scroll depth and the 20 most clicked labels; no property or query string) are kept as long as the project exists, so older periods can still be compared (D-026). Recordings: 30 days after their last chunk. Uptime checks: 7 days. Job check-ins (time, ok or error, duration; no personal data): the last 50 per job, as long as the job exists.
- Backups and the S3 copy follow your own schedule and retention.

## Rights of the people you measure

- **Access:** a person's profile → *Export their data (JSON)* (admins), or `GET /p/{project}/people/{id}/export.json` with the admin token. Each export is in the audit log.
- **Erasure:** a person's profile → *Forget this person* (admins), or `DELETE /api/projects/{project}/people/{id}/forget`. Their events (and those of the anonymous ids linked to them by `identify`) are rewritten out of the files at once, their recordings and profile are deleted, and anything they send later is dropped. They are removed from the daily rollups too, including the days older than 400 days; only counts with no one in them remain. The list of forgotten ids stays in the metadata, so a restore from an older backup is cleaned again at the next compaction (within 10 minutes). With S3 set, event days older than 120 days live on S3 only: forgetting someone rewrites those days too (within the hour), and their export reads them back. Other S3 copies and backups age out under your own retention.

## Three profiles

*This page is not legal advice.* It says what OpenLens does in each profile, so that you, your DPO or your lawyer can decide which one fits. `scripts/e2e/browser.mjs` checks the device side of the first two every night: cookies, `localStorage`, replay requests and the id sent.

| | **Exempt** | **Standard** | **Full** |
|---|---|---|---|
| Script tag | `data-cookieless data-replay="off"` | `data-replay="off"` | (nothing to add) |
| On the device | nothing: no cookie, no `localStorage` | `localStorage`: anonymous id, session, what `identify` was given, flags, answered surveys | the same as Standard |
| Visitor id | a daily hash on the server: a visitor can't be followed from one day to the next | a random id kept on the device, linked to the user on `identify` | the same as Standard |
| Session recordings | none | none | yes: inputs and texts masked unless the app unmasks them |
| `/flags` query string | `plan`, `country` and the `properties` given to `identify`, if you call it (not stored) | the same | the same |
| Typical basis | designed to meet the conditions of the CNIL audience-measurement exemption (below); you check that they hold | consent, where `localStorage` is treated like a cookie | consent |

**Exempt, the conditions to check** (CNIL, audience measurement exempt from consent). OpenLens is designed to meet them, but only you can confirm they hold for your site:
- the purpose is limited to audience measurement for the site itself (no cross-site tracking, no advertising, no data shared with others);
- nothing is stored on the device, and the visitor id lasts one day;
- the data is not used to follow a single person, which means no `identify` on pages measured this way, or a reasoned assessment if you call it;
- no session is recorded;
- visitors are told about the measurement (your privacy notice) and can object;
- the data is kept for a limited time (events 400 days here, D-015: check it against your own rule).

Templates to copy: a [record of processing](privacy-record-of-processing.md) and a [data-processing clause](privacy-dpa-clause.md) for a provider who hosts OpenLens for you.

## What Ask sends to the model

Ask (off until an admin configures a model) sends the configured provider a project brief (feature map, headline KPIs, latest releases, top insights, goals), the question and the six previous ones, and the results of the tools it calls: tables of at most 20 rows, from an aggregate-only view where people are counted, never listed, and groups of fewer than 5 are left out (D-021). It never sends event properties, page URLs, replays, emails, or user or session ids, unless an admin allows person-level detail for that project. Admins can see the exact last request on the Ask page. The provider's own retention applies to what it receives; a local model (Ollama, through an OpenAI-compatible URL) keeps everything on your network. Details: [docs/ai.md](ai.md).

## Audience measurement without consent

Some EU regulators exempt audience measurement from consent when it stores nothing on the device, keeps no person-level trail and records no session. With OpenLens, that is the cookieless mode below with replay off: `<script src=".../c.js" data-key="pk_…" data-cookieless data-replay="off">`. Whether it applies to you is for your DPO or lawyer to decide; this page is not legal advice.

The SDK also sends nothing when the browser sends Do Not Track or Global Privacy Control.

## Cookieless mode

Add `data-cookieless` to the script tag (or `init({ cookieless: true })`). The SDK then writes nothing to the device (no cookie, no localStorage; its state stays in memory), and the server turns each anonymous visitor into a hash of a salt that changes every UTC day, the project, the address and the user agent; the address is not stored, and yesterday's salt is overwritten, so a visitor can't be followed from one day to the next. A session continues while the visitor is active within 30 minutes. Logged-in users keep their own id, but the daily id is never linked to it. Session replay is off in this mode.

What it changes: anonymous visitors count as new every day (returning visitors, retention and churn rely on logged-in users), and behind no proxy (no `X-Forwarded-For`) every request counts as a new visitor.
