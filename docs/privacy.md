# Privacy

What OpenLens stores, for how long, where, and what it never stores. Everything stays on your server (and on the S3 copy, if you set one).

## Stored

- **Events** sent by your app: name, user id (`distinct_id`), session id, time, page URL and path, feature, role, account, plan, environment, release, active flags and the properties you send. Page views also get the **country** (looked up from the address, which is then dropped), **device class, browser and OS** (parsed from the user agent, which is then dropped), and the **referrer's host** (never its path or query).
- **Session recordings** (replays), with every input masked and the text of elements marked `data-openlens-mask` hidden.
- **People:** role, account, plan, first and last seen; role history.
- **Dashboard users:** email, name, role, time zone, last sign-in. Login links and sessions are signed; secrets (S3) are sealed.
- **Audit log** of admin actions (who, when, what), kept for the life of the install.

## Never stored

- IP addresses (used for the country and the rate limit, then dropped).
- Raw user agents, referrer paths and query strings.
- Input values in recordings.

## Retention (fixed, D-015)

- Events: 400 days. Recordings: 30 days after their last chunk. Uptime checks: 7 days.
- Backups and the S3 copy follow your own schedule and retention.

## Rights of the people you measure

- **Access:** a person's profile → *Export their data (JSON)* (admins), or `GET /p/{project}/people/{id}/export.json` with the admin token. Each export is in the audit log.
- **Erasure:** a person's profile → *Forget this person* (admins), or `DELETE /api/projects/{project}/people/{id}/forget`. Their events (and those of the anonymous ids linked to them by `identify`) are rewritten out of the files at once, their recordings and profile are deleted, and anything they send later is dropped. The list of forgotten ids stays in the metadata, so a restore from an older backup is cleaned again at the next compaction (within 10 minutes). The S3 copy and backups age out under your own retention.

## Cookieless mode

Planned (#273): no persistent identifier for anonymous visitors, for sites that run without a consent banner.
