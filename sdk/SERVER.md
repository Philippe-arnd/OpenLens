# Server SDKs

Thin clients for events that happen on the server: a payment confirmed, an export generated, a job that finished. They are also for flags checked server-side. They have no dependencies, batch their events, and never throw into your app.

| Language | Path | Install (once published) |
| --- | --- | --- |
| Node ≥ 18 | `sdk/node` | `npm install @openlens/node` |
| Python ≥ 3.9 | `sdk/python` | `pip install openlens` |
| Go ≥ 1.21 | `sdk/go` | `go get github.com/Philippe-arnd/OpenLens/sdk/go` |
| Ruby ≥ 3.0 | `sdk/ruby` | `gem install openlens` |
| PHP ≥ 8.1 | `sdk/php` | `composer require openlens/openlens` |

Until they are published, copy the single source file: `sdk/node/src/index.js`, `sdk/python/openlens/__init__.py`, `sdk/go/openlens.go`, `sdk/ruby/lib/openlens.rb` or `sdk/php/src/OpenLens.php`. The Go module can already be fetched from the repository.

```js
import { OpenLens } from "@openlens/node";
const ol = new OpenLens({ host: process.env.OPENLENS_HOST, key: process.env.OPENLENS_KEY, release: process.env.GIT_SHA });
ol.identify(user.id, { role: user.role, account: user.orgId });
ol.capture(user.id, "invoice_sent");
if (await ol.isEnabled("new-billing", user.id, { role: user.role })) { /* … */ }
// Flag rules on plan or other properties (never stored):
await ol.isEnabled("pro-export", user.id, { plan: "pro", properties: { seats: 12 } });
await ol.shutdown(); // on exit
```

```python
from openlens import OpenLens
ol = OpenLens(host=os.environ["OPENLENS_HOST"], key=os.environ["OPENLENS_KEY"], release=os.environ.get("GIT_SHA"))
ol.capture(str(user.id), "invoice_sent")
```

```go
ol, _ := openlens.New(openlens.Config{Host: os.Getenv("OPENLENS_HOST"), Key: os.Getenv("OPENLENS_KEY"), Release: os.Getenv("GIT_SHA")})
defer ol.Shutdown()
ol.Capture(userID, "invoice_sent", nil)
if ol.IsEnabled(ctx, "new-billing", userID, openlens.Traits{Role: role}) { /* … */ }
```

```ruby
OL = OpenLens::Client.new(host: ENV["OPENLENS_HOST"], key: ENV["OPENLENS_KEY"], release: ENV["GIT_SHA"])
OL.capture(user.id.to_s, "invoice_sent")
at_exit { OL.shutdown }
```

```php
$ol = new \OpenLens\OpenLens(getenv('OPENLENS_HOST'), getenv('OPENLENS_KEY'), ['release' => getenv('GIT_SHA') ?: null]);
$ol->capture((string) $user->id, 'invoice_sent'); // sent at the end of the request
```

PHP has no background thread: events are sent when 50 are queued and at the end of the request. Under PHP-FPM, that happens after the response if the app calls `fastcgi_finish_request()`.

## Server errors and route stats (Node)

```js
import express from "express";
import { OpenLens } from "@openlens/node";
const ol = new OpenLens({ host: process.env.OPENLENS_HOST, key: process.env.OPENLENS_KEY });
const app = express();
app.use(ol.express({ user: (req) => req.user?.id })); // first: route stats + trace context
// … routes …
app.use(ol.expressErrors()); // last: reports the error, then next(err)
ol.captureUncaught(); // optional: crashes and unhandled rejections
```

- **Route stats:** each request counts in its route template (`/api/entries/:id`, never the raw URL) and status class, in 10-minute windows: count, 5xx, duration histogram. Other frameworks call `ol.recordRequest({ method, route, status, ms })`.
- **Errors:** `ol.captureException(err, { distinctId, route })` anywhere else. A server error is grouped with the other errors, alerted on, and judged per release.
- **Session link:** the browser SDK adds `traceparent` and `baggage` to requests to its own origin, so a server error carries the session (and its replay) and the user. An API on another origin needs `traceOrigins: ["https://api.example.com"]` in the browser config, and its CORS must allow the `traceparent` and `baggage` headers.
- **Release:** keep the same release name on both sides. Both SDKs read the host's commit (`SOURCE_COMMIT` on Coolify) when `release` is not set.
- **Volume:** a server sends one batch every 5 s and one stats event per route and status class every 10 minutes, far below the capture limits, whatever the traffic.

Use the same `distinct_id` as the browser SDK's `identify()`, so server and browser events belong to the same person. Success events sent from the server count for the feature map like any other event.
