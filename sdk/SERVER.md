# Server SDKs

Thin clients for events that happen on the server: a payment confirmed, an export generated, a job that finished. They are also for flags checked server-side. They have no dependencies, batch their events, and never throw into your app.

| Language | Path | Install (once published) |
| --- | --- | --- |
| Node ≥ 18 | `sdk/node` | `npm install @openlens-obs/node` |
| Python ≥ 3.9 | `sdk/python` | `pip install openlens` |
| Go ≥ 1.21 | `sdk/go` | `go get github.com/Philippe-arnd/OpenLens/sdk/go` |
| Ruby ≥ 3.0 | `sdk/ruby` | `gem install openlens` |
| PHP ≥ 8.1 | `sdk/php` | `composer require openlens/openlens` |

Until they are published, copy the single source file: `sdk/node/src/index.js`, `sdk/python/openlens/__init__.py`, `sdk/go/openlens.go`, `sdk/ruby/lib/openlens.rb` or `sdk/php/src/OpenLens.php`. The Go module can already be fetched from the repository.

```js
import { OpenLens } from "@openlens-obs/node";
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

## Server errors, route stats and runtime health (Node)

```js
import express from "express";
import { OpenLens } from "@openlens-obs/node";
const ol = new OpenLens({ host: process.env.OPENLENS_HOST, key: process.env.OPENLENS_KEY });
const app = express();
app.use(ol.express({ user: (req) => req.user?.id })); // first: route stats, runtime health, trace context
// … routes …
app.use(ol.expressErrors()); // last: reports the error, then next(err)
ol.captureUncaught(); // optional: crashes and unhandled rejections
```

- **Route stats:** each request counts in its route template (`/api/entries/:id`, never the raw URL) and status code, in one stats window (see **Stats window** below), as one data point of OpenTelemetry's `http.server.request.duration` histogram: the semantic-convention attributes `http.request.method`, `http.route`, `http.response.status_code`, seconds, and the default explicit bounds (5 ms to 10 s). Every request counts: nothing is sampled. Other frameworks call `ol.recordRequest({ method, route, status, ms })`.
- **Health checks are not route stats** (D-100): requests to `/health`, `/healthz`, `/health/*` (any route under `/health/`), `/api/health`, `/api/healthz`, `/livez`, `/readyz`, `/ready`, `/live`, `/ping` and `/status` (the route template, without case and with or without a trailing slash) are not counted, so the SDK sends nothing for them. The OpenLens server also drops the route stats of these routes, and of the project's own uptime health path (Settings › Alerts, unless it is `/`), from any SDK and from OTLP. A server error (`$error`) on a health route is still kept: a failing health check matters. There is no switch.
- **Exemplars:** each route stats point also carries up to two real requests, OpenTelemetry's *exemplars*: the **slowest** request of its route, status code and window, and for a 5xx code the **first** one. An exemplar holds the duration, the time, the `trace_id` (when the app's OpenTelemetry sent `traceparent`), and the session (`x-openlens-session`), and nothing else. **Never kept:** the user (the session leads to them, and is erased with them), the request body, its headers, cookies, URL or query string. The routes API and MCP `routes` return each route's `slowest` and `first_5xx` of the period with their session, so the replay opens at that time; a release whose route got slower names its slowest request in the verdict. Other frameworks pass `traceId` and `sessionId` to `recordRequest`.
- **Errors:** `ol.captureException(err, { distinctId, route })` anywhere else. A server error is grouped with the other errors, alerted on, and judged per release.
- **Session link:** the browser SDK adds one header, `x-openlens-session`, to requests to its own origin, so a server error carries the session and its replay. An API on another origin needs `traceOrigins: ["https://api.example.com"]` in the browser config, and its CORS must allow that header. No user id travels in a header: pass `user` to `express()`.
- **OpenTelemetry:** OpenLens reads W3C trace context and never writes it. When the app's own OpenTelemetry sends `traceparent`, the trace id lands on server errors and failed requests as `trace_id`, so you can open the same request in your tracing tool. OpenLens adds no `traceparent` and no `baggage`: your server keeps its own sampling, and nothing of OpenLens is forwarded to other services.
- **Runtime health:** `express()` also sends `$runtime_stats` (other frameworks call `ol.captureRuntime()` once at start-up). At start, one event with `process.start: true`, sent by the next flush; then every 10 minutes, whatever the stats window (D-100), one event with OpenTelemetry's runtime names: `nodejs.eventloop.delay.p50` and `.p99` (seconds, from `perf_hooks.monitorEventLoopDelay`, over those 10 minutes), and their largest `process.memory.usage` (RSS) and `v8js.memory.heap.used` (bytes), sampled on each flush. No new timer, nothing keeps the process alive, `shutdown()` sends the open window. A release's verdict turns regressed only when its process restarts per day go up significantly (a crash loop); memory and loop delay are shown per release, never judged. There is no switch to turn it off: it costs one small event per process every 10 minutes, 144 a day, plus one per start (D-087, D-100).
- **Outgoing calls (opt-in):** `ol.captureOutgoing()` once at start-up counts the app's own HTTP calls to other services (the global `fetch` and `http`/`https`, through Node's diagnostics channels) per external host: one `$client_stats` per method, host and status code every stats window, as OpenTelemetry's `http.client.request.duration` histogram (`server.address`, `http.request.method`, `http.response.status_code`, and `error.type`, the error's code such as `ECONNREFUSED`, when a call got no response). **Recorded:** the host name, the method, the status code or error code, the duration to the response headers. **Never recorded:** the path, the query, the port, headers, bodies, credentials or error messages; calls to the OpenLens host itself; more than 50 hosts per window (the rest count as `_OTHER`). A call fails on a 5xx or no response. The verdict shows a dependency that fails more or got slower than on the release before, but never judges it; a host failing for 15 minutes raises a `dep:<host>` alert (under *failing requests*). Calls made outside `fetch` and `http`/`https` (raw sockets, native gRPC clients) are not seen. Apps on OpenTelemetry send `http.client.request.duration` over OTLP instead (D-090).
- **Release:** keep the same release name on both sides. Both SDKs read the host's commit (`SOURCE_COMMIT` on Coolify) when `release` is not set.
- **Stats window:** route stats and outgoing calls are counted in windows of 10 minutes by default (runtime health always in 10-minute windows, D-100). The OpenLens server picks the window from its disk budget, hourly: 1 or 5 minutes when the room left after the floor (D-088) holds the extra stats points for 400 days and the busiest project has few active routes, else 10 minutes. It says so in every capture response, `x-openlens-stats-window: 60` (seconds), and the SDK follows it: on the next flush the windows being filled are sent with their own length, and new ones use the new length, aligned on it. Each stats event says its window in `$window_s`, and OpenLens reads any mix of windows, so totals, alerts and verdicts do not change with it. To pin one window, pass `statsWindowSeconds: 60`, `300` or `600` to `new OpenLens(...)` (anything else is ignored); it wins over the header (D-091). `GET /api/capacity` (admin) and the MCP `capacity` tool show the window in force and why (`stats_window`).
- **Volume:** a server sends one batch every 5 s and one stats event per route and status code per window, every 10 minutes by default (and one runtime event per process every 10 minutes, and with `captureOutgoing()` one per host, method and status code), far below the capture limits, whatever the traffic.

Use the same `distinct_id` as the browser SDK's `identify()`, so server and browser events belong to the same person. Success events sent from the server count for the feature map like any other event.

## Job check-ins

A cron job, a queue worker or a nightly export that stops running sends no request and raises no error. Make it check in at the end of each run, and OpenLens alerts on the project's channels when a run is **missed** (no check-in for 1.5 times its usual gap), **failed** (the check-in said `error`) or **slower** (twice its usual duration or more). Each alert is sent once and closes with the next check-in that clears it. The jobs are listed on the Uptime page.

From a shell script or a crontab line:

```sh
0 3 * * * /opt/app/export.sh && curl -fsS -X POST "https://lens.example.com/check-in/$OPENLENS_KEY/nightly-export"
# a failure, with the run's duration in milliseconds:
curl -fsS -X POST "https://lens.example.com/check-in/$OPENLENS_KEY/nightly-export?status=error&ms=81234"
```

From a server SDK (the key travels in the `X-OpenLens-Key` header, `POST /check-in/<slug>`):

```js
await ol.checkIn("nightly-export", { status: "ok", ms: Date.now() - started });
```

```python
ol.check_in("nightly-export", status="ok", ms=elapsed_ms)
```

```go
ol.CheckIn(ctx, "nightly-export", openlens.CheckInOptions{Duration: time.Since(start)})
```

```ruby
OL.check_in("nightly-export", status: "ok", ms: elapsed_ms)
```

```php
$ol->checkIn('nightly-export', ['status' => 'ok', 'ms' => $elapsedMs]);
```

- **Schedule:** learned, no cron expression to type: the median gap between the last check-ins, once there are four. One long pause or a manual rerun does not move it. To give it instead, add `expectEvery` (seconds, 10 to 31 days) to a check-in (`expect_every` in Python and Ruby, `ExpectEvery` in Go); `expectEvery=0` goes back to learning.
- **Slug:** 1 to 64 lowercase letters, digits, `_` or `-`. One check-in per job every 10 seconds (a 429 with `Retry-After` beyond); at most 200 jobs per project (remove one on the Uptime page).
- **Never in the way:** the SDK call is sent at once, returns false on any error and never throws. The `curl` line's `-f` makes a refused check-in fail the cron line, so the cron's own mail shows it.
- **Key in the path:** the `curl` form puts the project key in the URL. OpenLens never logs it, but a reverse proxy in front may: prefer the header form (`curl -H "X-OpenLens-Key: $OPENLENS_KEY" -X POST https://lens.example.com/check-in/nightly-export`) when your proxy logs paths.
- **Availability:** jobs are not counted in the Uptime page's availability unless an admin ticks *Count jobs in availability* on the Jobs card. Then each check-in counts as a check and each missed run as a failed one, in the last 24 hours and 7 days.

## Already on OpenTelemetry

An app whose server already exports OpenTelemetry metrics needs no OpenLens SDK for its route stats: point an OTLP/HTTP exporter at OpenLens with one header, the project key. OpenLens reads the `http.server.request.duration` histogram (method, route template, status code) and stores each data point as the Node SDK's route stats, so the routes table, the verdict and the alerts work the same. Data points of health-check routes (see **Health checks are not route stats** above) are accepted and not stored. Resource `service.version` is the release and `deployment.environment.name` the environment. Metrics only: no spans, no traces.

With an OpenTelemetry Collector (protobuf and gzip, its defaults):

```yaml
exporters:
  otlp_http/openlens:          # `otlphttp` in older Collectors
    endpoint: https://lens.example.com   # /v1/metrics is appended
    headers:
      x-openlens-key: pk_your_project_key
service:
  pipelines:
    metrics:
      receivers: [otlp]
      exporters: [otlp_http/openlens]
```

With the Node SDK's `@opentelemetry/exporter-metrics-otlp-http` (JSON), through the standard variables:

```sh
OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=https://lens.example.com/v1/metrics
OTEL_EXPORTER_OTLP_METRICS_HEADERS=x-openlens-key=pk_your_project_key
OTEL_RESOURCE_ATTRIBUTES=service.version=$GIT_SHA,deployment.environment.name=production
OTEL_SEMCONV_STABILITY_OPT_IN=http   # HTTP instrumentations that still emit the old http.server.duration
```

- **Exemplars:** of a point's exemplars, OpenLens keeps the slowest and, for a 5xx point, the first, with their trace id. Of their filtered attributes it keeps only `session.id`; every other attribute is dropped, `user.id` and `enduser.id` included (an exemplar keeps the session, never the user). Put the OpenLens session id (the browser's `x-openlens-session` header) in `session.id` to link a request to its replay.
- **Both temporalities:** cumulative (every SDK's default) and delta. For cumulative points OpenLens stores the difference with the last point it saw; an app restart (a new start time) starts over, and a retried export adds nothing.
- **Any bucket bounds:** OpenTelemetry's defaults are read as they are; other bounds are folded into them by upper bound, so p95 is never under-reported. Units `s` and `ms` both work.
- **Outgoing calls:** `http.client.request.duration` is read too, as outgoing dependencies per host (#608); its data points need a host in `server.address`.
- **Everything else is dropped and counted:** other metrics come back in the response's `partialSuccess.rejectedDataPoints`. A data point of `http.server.request.duration` needs `http.route`.
- **Limits:** 4 MB per request and 10,000 data points; the capture rate limits apply (one data point counts as one event). OTLP over gRPC is not served: let the Collector convert.
