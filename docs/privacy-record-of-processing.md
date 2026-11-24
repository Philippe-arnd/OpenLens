# Record of processing: template

*A starting point for the record of processing activities (GDPR art. 30) of a site measured with OpenLens. It is not legal advice: adapt every line to your situation, and keep the profile you chose in [privacy.md](privacy.md#three-profiles) in mind.*

| Field | What to write |
|---|---|
| **Processing** | Product analytics of `<your app>`: which features are used, by which roles, after which release. |
| **Controller** | `<your organisation, address, contact>` |
| **DPO** | `<name and contact, if you have one>` |
| **Purposes** | Measuring use of the application to improve it; detecting errors and regressions; (Full profile) understanding problems by watching masked session recordings. |
| **Legal basis** | Exempt profile: audience measurement exempt from consent, if its conditions hold (privacy.md). Standard and Full: consent, or legitimate interest after a balancing test, as your counsel decides. |
| **People concerned** | Visitors and users of `<your app>`. |
| **Data** | Event names, pseudonymous ids (a daily hash in the Exempt profile), session ids, times, page paths without query strings (except `utm_*`, `ref`, `gclid`), features, role/account/plan if you send them, country, device class, browser and OS, referrer host. Full profile: masked session recordings. Never: IP addresses, raw user agents, input values. |
| **Recipients** | Your team (dashboard roles: admin, viewer). If Ask is on: the model provider you configured (aggregates only, unless person-level detail is allowed). If the S3 copy is on: your storage provider. |
| **Transfers outside the EU** | None by OpenLens itself. Check your host, your S3 provider and, if Ask is on, your model provider. |
| **Retention** | Events 400 days, recordings 30 days after their last chunk, uptime checks 7 days (fixed, D-015); backups as you schedule them. |
| **Security** | Self-hosted on `<your server>`; HTTPS; dashboard access by signed sessions and roles; secrets sealed; audit log of admin actions; backups as you schedule them. |
| **Rights** | Access: export from a person's profile. Erasure: *Forget this person*. Objection: Do Not Track / Global Privacy Control are honoured, and `setConsent(false)` stops sending. |
