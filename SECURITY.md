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

## Supported versions

Only the **latest minor release** gets security fixes, as a patch release (for example, when 0.3 is current, 0.3.x gets the fix and 0.2 does not). Stable releases ship every week (see the [release train](docs/development.md#releasing)), and an upgrade within the same major keeps your data, so staying current is the supported path.

| Version | Supported |
|---|---|
| latest minor (`0.x`, the newest) | ✅ |
| older minors | ❌ |

## How we treat input

OpenLens exposes an internet-facing ingestion endpoint, so we treat every byte it receives as hostile. Input sizes are bounded, parsing never panics, and every crash fix comes with a regression input. The parsers of what strangers send are fuzzed nightly (`fuzz/`).
