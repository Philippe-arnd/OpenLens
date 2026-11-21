# Security

Report vulnerabilities privately through GitHub's **private security advisory** on this repository ("Security" tab, then "Report a vulnerability"). Never open a public issue for one.

OpenLens exposes an internet-facing ingestion endpoint, so we treat every byte it receives as hostile: input sizes are bounded, parsing never panics, and every crash fix comes with a regression input.
