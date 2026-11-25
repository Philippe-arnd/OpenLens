# Security

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's **private security advisory** on this repository: open the "Security" tab, then "Report a vulnerability". Never open a public issue for one.

Please include the version (the image tag or the release you installed), what an attacker sends, and what happens.

## What happens next

| Step | When |
|---|---|
| Acknowledgement of your report | within 3 business days |
| First assessment (is it a vulnerability, and how severe) | within 7 days |
| Fix released, for confirmed issues | as a patch release of the latest minor, as fast as the severity requires |
| Public disclosure | coordinated with you: when the fix is out, or 90 days after the report, whichever comes first |

You are credited in the advisory unless you ask not to be.

## Passwords

Dashboard passwords (#428) are stored only as Argon2id hashes (a random salt each) in `meta.db`, and never logged. Failures never lock an account (no one can lock someone else out): after 3 failed tries in 15 minutes, each new try for that email waits 1 s, then 2 s, 4 s… up to 30 s, one try at a time; one client address gets at most 30 failures per 15 minutes, whatever the emails. Wrong *current* passwords on *My profile* count the same way. Every failure gets the same answer, whether the account exists or not, and goes to the audit log. These counters live in memory, so a restart clears them. Changing a password signs out that user's other sessions. An admin can reset a member's password from *Team*: the old password stops working, their sessions end, and the admin gets a one-time link (24 h) for them to set a new one; no password is ever shown or sent. The admin token stays the recovery path.

## Second factor

Each account with a password can turn on an authenticator app (#440) on *My profile → Sign-in*: after a correct password, sign-in asks for its 6-digit code (RFC 6238: HMAC-SHA1, 30 s steps, one step of clock drift either way), or for one of 8 recovery codes shown once when it is turned on.

- **Scope.** It guards **password sign-in only**. Email login links and single sign-on (OIDC) are separate sign-in paths and do not ask for it, and the **admin token** stays the break-glass path. An account that can also get a login link or sign in through the identity provider is only as strong as those paths.
- **Storage.** The secret is sealed with the server key (ChaCha20-Poly1305: `OPENLENS_SECRET_KEY`, else `<data>/.secret.key`), so a copy of `meta.db` alone does not reveal it. Recovery codes are stored only as SHA-256 hashes. Backups do not carry the key: **a store restored without the key loses its second factors** (the codes no longer check); the admin token or an admin's reset is the way back in.
- **Codes.** Each 30 s step works once (the last step used is stored), and each recovery code works once.
- **The code step.** After the password, the browser gets a signed challenge (5 minutes, bound to the account and its current credentials), not a session. Code attempts go through the same slow-down and per-address budget as passwords, and a correct password does not reset them; failures and successes go to the audit log.
- **Changes.** Turning it on needs the current password and a code; turning it off needs the current password and a code or a recovery code. Both sign out the user's other sessions. An admin's password reset clears the password and the second factor together.

## Knowing you are affected

Fixed vulnerabilities are published as advisories on the repository's **Security** tab (affected and patched versions). The release that fixes one carries a **Security** section in its notes and in `CHANGELOG.md`, and a running OpenLens that sees such a release shows *Security update available* in every page's footer instead of the plain update notice (turn the daily check off with `OPENLENS_UPDATE_CHECK=off`).

## Supported versions

Only the **latest minor release** gets security fixes, as a patch release (for example, when 0.3 is current, 0.3.x gets the fix and 0.2 does not). Stable releases ship every week (see the [release train](docs/development.md#releasing)), and an upgrade within the same major keeps your data, so staying current is the supported path.

| Version | Supported |
|---|---|
| latest minor (`0.x`, the newest) | ✅ |
| older minors | ❌ |

## How we treat input

OpenLens exposes an internet-facing ingestion endpoint, so we treat every byte it receives as hostile. Input sizes are bounded, parsing never panics, and every crash fix comes with a regression input. The parsers of what strangers send are fuzzed nightly (`fuzz/`).
