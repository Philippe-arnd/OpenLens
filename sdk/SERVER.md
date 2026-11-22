# Server SDKs

Thin clients for events that happen on the server: a payment confirmed, an export generated, a job that finished. They are also for flags checked server-side. They have no dependencies, batch their events, and never throw into your app.

| Language | Path | Install (once published) |
| --- | --- | --- |
| Node ≥ 18 | `sdk/node` | `npm install @openlens/node` |
| Python ≥ 3.9 | `sdk/python` | `pip install openlens` |

Until they are published, copy the single source file (`sdk/node/src/index.js` or `sdk/python/openlens/__init__.py`).

```js
import { OpenLens } from "@openlens/node";
const ol = new OpenLens({ host: process.env.OPENLENS_HOST, key: process.env.OPENLENS_KEY, release: process.env.GIT_SHA });
ol.identify(user.id, { role: user.role, account: user.orgId });
ol.capture(user.id, "invoice_sent");
if (await ol.isEnabled("new-billing", user.id, { role: user.role })) { /* … */ }
await ol.shutdown(); // on exit
```

```python
from openlens import OpenLens
ol = OpenLens(host=os.environ["OPENLENS_HOST"], key=os.environ["OPENLENS_KEY"], release=os.environ.get("GIT_SHA"))
ol.capture(str(user.id), "invoice_sent")
```

Use the same `distinct_id` as the browser SDK's `identify()`, so server and browser events belong to the same person. Success events sent from the server count for the feature map like any other event.
