# Data-processing clause: template

*For a provider who hosts and runs OpenLens on behalf of a customer (GDPR art. 28). A starting point, not legal advice: have it reviewed, and attach it to your contract.*

1. **Subject and duration.** The Provider hosts and operates an OpenLens instance for the Customer's product analytics for the duration of the contract.
2. **Nature and purpose.** Storing and processing the analytics events the Customer's applications send, and showing them to the Customer's users in the dashboard, only to measure and improve those applications.
3. **Data and people.** The data listed in the Customer's record of processing (see [the template](privacy-record-of-processing.md)), about the visitors and users of the Customer's applications.
4. **Instructions.** The Provider processes the data only on the Customer's documented instructions, including the profile chosen in [privacy.md](privacy.md#three-profiles) and the optional features turned on (Ask, the S3 copy, the digest, alerts).
5. **Confidentiality.** People who can reach the instance or its backups are bound to confidentiality.
6. **Security.** HTTPS, signed sessions and roles, sealed secrets, the audit log, backups and their tests; the Provider keeps the instance on a supported release (latest minor, [SECURITY.md](../SECURITY.md)).
7. **Sub-processors.** The Provider lists its hosting, storage and (if Ask is on) model providers, and tells the Customer before adding or changing one.
8. **Rights of the people.** The Provider helps the Customer answer access and erasure requests (export and *Forget this person*) without undue delay.
9. **Breaches.** The Provider tells the Customer without undue delay, and within `<48> hours`, of a personal-data breach.
10. **End of contract.** The Provider returns the data (an `openlens backup` copy) or deletes it, including backups and the S3 copy, at the Customer's choice, and confirms it in writing.
11. **Audits.** The Provider makes available what is needed to show compliance, and allows audits on reasonable notice.
