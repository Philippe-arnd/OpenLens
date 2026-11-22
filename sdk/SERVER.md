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

Use the same `distinct_id` as the browser SDK's `identify()`, so server and browser events belong to the same person. Success events sent from the server count for the feature map like any other event.
