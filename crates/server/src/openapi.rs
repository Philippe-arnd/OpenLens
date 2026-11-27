//! The API contract (#312): every `/api` route with its method, a summary
//! and the token scope it needs, served as OpenAPI 3.1 at
//! `/api/openapi.json` (and `/api/v1/openapi.json`). A test checks the
//! table against the routes in the source, both ways.
//!
//! `/api/v1/...` is the versioned prefix; `/api/...` stays as an alias.

use axum::Json;
use serde_json::{Map, Value, json};

/// `(method, path, scope, summary)` of every API route.
pub(crate) const ROUTES: &[(&str, &str, &str, &str)] = &[
    (
        "GET",
        "/api/projects",
        "read",
        "List projects and their capture keys.",
    ),
    (
        "POST",
        "/api/projects",
        "admin",
        "Create a project; returns its key and snippet.",
    ),
    (
        "PATCH",
        "/api/projects/{project}",
        "admin",
        "Rename a project, set its time zone or its environment settings (host rules, aliases, hidden environments, checks per environment).",
    ),
    (
        "GET",
        "/api/projects/{project}/environments",
        "read",
        "Hosts per environment, host rules, unclassified hosts, aliases, hidden environments and checks per environment.",
    ),
    (
        "POST",
        "/api/projects/{project}/environments",
        "admin",
        "Re-label a host's past events (`host`, `days` up to 90, `dry_run`) with the current host rules; returns the moves per old and new environment.",
    ),
    (
        "POST",
        "/api/projects/{project}/environments/allow",
        "admin",
        "Preview a host allow-list (`hosts`, exact or `*.suffix`, at most 50; empty turns it off) over the last 24 h: the hosts outside it and their events. With `confirm: true` the list is saved; it drops other hosts' events only while a listed host has sent events in the last 7 days.",
    ),
    (
        "GET",
        "/api/projects/{project}/vitals",
        "read",
        "Web Vitals at p75 with thresholds, worst pages, likely cause and ranked actions.",
    ),
    (
        "DELETE",
        "/api/projects/{project}",
        "admin",
        "Delete a project and all its data (`?confirm=`).",
    ),
    (
        "POST",
        "/api/projects/{project}/sql",
        "read",
        "Run read-only SQL on `events`.",
    ),
    (
        "GET",
        "/api/projects/{project}/board",
        "read",
        "The Focus board as JSON: its period blocks since the current release, against the same length before (`window=fixed`: the fixed 7-, 14- and 30-day windows). A date range or past release (`from`/`to`, `since`) reading more than 30 days of events answers 409 with its `estimate` until asked with `confirm=1`.",
    ),
    (
        "GET",
        "/api/projects/{project}/board/estimate",
        "read",
        "What the board's window would read, with the board's parameters: days from the daily rollups and from the events, bytes, rows, a rough time, and whether it needs `confirm=1` (`null` for the fixed windows).",
    ),
    (
        "POST",
        "/api/projects/{project}/board/jobs",
        "read",
        "Compute the board in the background, with the board's parameters in the query (`from`/`to`, `since`, filters): 202 with the job's `id` and `url`. 429 when the caller already has a job queued or running (its id in `job`) or the server runs as many as it takes (2 at once, 8 queued or running). A job is its starter's alone and kept an hour once finished.",
    ),
    (
        "GET",
        "/api/projects/{project}/board/jobs/{job}",
        "read",
        "A board job: `state` (queued, running, done, failed, cancelled), `progress` (0 to 1; while running, the time spent against the estimate), `elapsed_secs`, `estimate_secs`, `error`, and once done the `board` (as `GET …/board` answers it) and its `revenue`. 404 for another caller's job.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/board/jobs/{job}",
        "read",
        "Cancel a board job: its queries stop. A finished job is left as it is. Answers the job.",
    ),
    (
        "GET",
        "/api/projects/{project}/errors",
        "read",
        "Error groups.",
    ),
    (
        "GET",
        "/api/projects/{project}/breakdown",
        "read",
        "Users, sessions and events by one dimension.",
    ),
    (
        "GET",
        "/api/projects/{project}/people/{distinct_id}",
        "read",
        "One person's profile and recent events.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/people/{who}/forget",
        "admin",
        "Erase a person everywhere.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map",
        "read",
        "The feature map in force.",
    ),
    (
        "PUT",
        "/api/projects/{project}/feature-map",
        "write",
        "Save a new feature map version.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map/health",
        "read",
        "Map health: score and defects.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map/relations",
        "read",
        "Relations inferred between features from 30 days of sessions: lift, which comes first, leads to value.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map/versions",
        "read",
        "Saved versions of the map.",
    ),
    (
        "POST",
        "/api/feature-map/validate",
        "read",
        "Check a feature map without saving it.",
    ),
    (
        "GET",
        "/api/projects/{project}/tickets",
        "read",
        "Issues opened from findings.",
    ),
    (
        "POST",
        "/api/projects/{project}/tickets",
        "write",
        "Open an issue from a finding.",
    ),
    (
        "POST",
        "/api/projects/{project}/import",
        "write",
        "Import past events (JSON lines).",
    ),
    (
        "GET",
        "/api/projects/{project}/export",
        "read",
        "Export one day as JSON lines (`?date=`).",
    ),
    (
        "GET",
        "/api/projects/{project}/flows",
        "read",
        "Feature flows.",
    ),
    (
        "GET",
        "/api/projects/{project}/status",
        "read",
        "Up, down, under maintenance, unknown or off; open incidents and maintenance windows running or within 7 days; production's `availability` (as `uptime/report`).",
    ),
    (
        "GET",
        "/api/projects/{project}/churn",
        "read",
        "Churn report and people at risk.",
    ),
    (
        "GET",
        "/api/projects/{project}/goals",
        "read",
        "Goals and their progress.",
    ),
    (
        "POST",
        "/api/projects/{project}/goals",
        "write",
        "Set or remove a goal.",
    ),
    (
        "GET",
        "/api/projects/{project}/releases",
        "read",
        "Releases per environment over 90 days (`env` for one): state (rolling out, live, superseded, rolled back; failed or deploying for a deploy with no traffic) from the share of daily users, first and last seen, users, time in staging, deploy time and last deploy status; the deploys the deploy hook recorded; when the hook was set up.",
    ),
    (
        "GET",
        "/api/projects/{project}/releases/{release}/report",
        "read",
        "One release (`env`: its environment, production by default) compared with the release before it: the verdict (healthy, regressed or inconclusive; none until 30 sessions and 8 users or 24 h) and its reasons, both releases' numbers, and every check (sessions with errors, bounce, tasks, new error groups, Web Vitals, features with a success event, goals and guardrails).",
    ),
    (
        "POST",
        "/api/projects/{project}/releases",
        "write",
        "Record a release from the deploy step (`{release, commit?, from?, environment?}`; `openlens-cli release`): a succeeded deploy with its commit range, from `from` or the environment's previous deployed commit to `commit`.",
    ),
    (
        "GET",
        "/api/projects/{project}/delivery",
        "read",
        "Delivery metrics per environment over `days` (1 to 90, default 90; `env` for one): deploy frequency (succeeded deploys from the deploy hook, else releases reaching live), lead time from first commit to live (unknown until a commit range is recorded), change failure rate (failed or rolled-back deploys and releases, and regressed verdicts) and time to restore, as medians in seconds.",
    ),
    (
        "GET",
        "/api/projects/{project}/routes",
        "read",
        "Server routes of one environment (`env`, production by default) over `days` (1 to 90, default 7), from the server SDKs' route stats: per method and route template, requests, 5xx count and share, p50 and p95 in ms (upper bounds of the duration histogram), the largest duration, the feature whose key action the route is (`server:` in the feature map), the p95 of each of the last ten days (from that day's merged histograms), and how the route moved from the release before to the latest one. Key actions come first, then the routes that got worse, then the largest change; the totals and the counts of worse routes and key actions come with the list.",
    ),
    (
        "POST",
        "/api/projects/{project}/deploy-hook",
        "write",
        "Make or rotate the project's deploy hook URL (`/hooks/deploy/<secret>`, shown in this response only; the previous URL stops working). The hook takes Coolify, Vercel, Netlify, Render, GitHub deployment_status, Argo CD, Flux and Fly notifications, or `{status, release?, commit?, environment?, at?}`.",
    ),
    (
        "GET",
        "/api/projects/{project}/timeline",
        "read",
        "Releases, map changes and notes.",
    ),
    (
        "POST",
        "/api/projects/{project}/timeline",
        "write",
        "Add a timeline note.",
    ),
    (
        "GET",
        "/api/projects/{project}/flags",
        "read",
        "Feature flags.",
    ),
    (
        "PUT",
        "/api/projects/{project}/flags/{key}",
        "write",
        "Create or change a flag.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/flags/{key}",
        "write",
        "Delete a flag.",
    ),
    (
        "PATCH",
        "/api/projects/{project}/flags/{key}",
        "write",
        "Switch an existing flag on or off (`enabled`) and/or set its rollout (`rollout`, 0-100); the rest is unchanged.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/goals/{goal}",
        "write",
        "Remove a goal.",
    ),
    (
        "POST",
        "/api/projects/{project}/objectives",
        "write",
        "Set an objective: `title`, one to three existing goal ids as key results (`goals`), and the last day of its period (`end`, YYYY-MM-DD, within a year).",
    ),
    (
        "DELETE",
        "/api/projects/{project}/objectives/{objective}",
        "write",
        "Remove an objective.",
    ),
    (
        "GET",
        "/api/projects/{project}/uptime",
        "read",
        "Status, availability over a day and a week (planned maintenance left out, and how long), response times, maintenance windows, incidents of 30 days and the last checks.",
    ),
    (
        "GET",
        "/api/projects/{project}/maintenance",
        "read",
        "Planned maintenance windows: planned, running and ended within 30 days (`windows`).",
    ),
    (
        "POST",
        "/api/projects/{project}/maintenance",
        "write",
        "Schedule a maintenance window: `start` (now, RFC 3339 or YYYY-MM-DD HH:MM in the project's zone; default now), `end` or `minutes` (at most 3 days), `components` (production, the default, and/or environments), `text`, `announce` (once on the alert channels). Its incidents are not announced and its time is not counted in availability.",
    ),
    (
        "POST",
        "/api/projects/{project}/maintenance/{window}/extend",
        "write",
        "Extend a planned or running window: a new `end`, or `minutes` added to its end.",
    ),
    (
        "POST",
        "/api/projects/{project}/maintenance/{window}/end",
        "write",
        "End a running window now; incidents still failing are announced from then.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/maintenance/{window}",
        "write",
        "Cancel a window that has not started.",
    ),
    (
        "GET",
        "/api/projects/{project}/uptime/days",
        "read",
        "Uptime history, one row per UTC day (`uptime.days`): `from` and `to` (YYYY-MM-DD, at most 400 days; default the last 90), `env` (default production). Per day: state (up, partial, down, none), availability, seconds up and measured, checks, failed checks, down incidents, p50/p95/max response time.",
    ),
    (
        "GET",
        "/api/projects/{project}/incidents",
        "read",
        "Every incident, newest first, page by page (`uptime.incidents`): `limit` (1 to 100, default 50) and `before` (the previous page's `next`).",
    ),
    (
        "GET",
        "/api/projects/{project}/uptime/report",
        "read",
        "Availability (`uptime.report`) of `env` (default production) over 24h, 7d, 30d, 90d, this month, last month (in the project's zone) and 365d: time up over time measured, seconds not measured (no check: neither up nor down) and of planned maintenance left out, incidents, p95; and `slo` when a target is set: target, 30-day availability, error budget, spent and left (seconds), burn rate over the last hour and day.",
    ),
    (
        "PUT",
        "/api/projects/{project}/uptime/target",
        "write",
        "Set the availability target of an `environment` (default production) over a rolling 30 days (`uptime.set_target`): `target` a share from 0.5 to 0.99999 (0.999 for 99.9%), or null to remove it.",
    ),
    (
        "GET",
        "/api/projects/{project}/announcements",
        "read",
        "Status announcements of the last `days` (default 90, at most 400), newest first, each with its dated updates oldest first; the pinned notice; what the Uptime page proposes about recent incidents (`write_update`, `mark_resolved`); whether they are sent to the alert channels.",
    ),
    (
        "POST",
        "/api/projects/{project}/announcements",
        "write",
        "Post an announcement: `title`, `body` (plain text, at most 4000 characters), `level` (information, degraded, outage, resolved), `components` (production or environment names), `start`/`end` (RFC 3339, Unix seconds or `YYYY-MM-DD HH:MM` in the project's zone), `pinned` (information only), `incident` (an id), `state` (its first update).",
    ),
    (
        "PATCH",
        "/api/projects/{project}/announcements/{id}",
        "write",
        "Change an announcement: only the fields given change (`end: \"\"` removes the end).",
    ),
    (
        "DELETE",
        "/api/projects/{project}/announcements/{id}",
        "write",
        "Delete an announcement and its updates.",
    ),
    (
        "POST",
        "/api/projects/{project}/announcements/{id}/updates",
        "write",
        "Post a dated update: `state` (investigating, identified, monitoring, resolved) and `body`. A resolved update also resolves and ends the announcement.",
    ),
    (
        "POST",
        "/api/projects/{project}/announcements/{id}/close",
        "write",
        "End an announcement now (Mark resolved), with an optional closing `body`.",
    ),
    (
        "PUT",
        "/api/projects/{project}/announcements/send",
        "write",
        "Also send announcements, their updates and their end to the alert channels (`send`, off by default).",
    ),
    (
        "GET",
        "/api/projects/{project}/status-page",
        "read",
        "The public status page's settings (#561): `enabled`, `slug`, `components` (production first, then monitored environments, each with its public `name` and `response_time`), `path` (`/status/<slug>`) and the `monitored` environments that can be shown. The page itself is `/status/<slug>` (also `.json`, `/badge.svg?window=state|24h|7d|30d|90d` and `/feed.atom`), public, without a token.",
    ),
    (
        "PUT",
        "/api/projects/{project}/status-page",
        "write",
        "Change the public status page: only the fields given change. `enabled` (needs a slug), `slug` (3 to 40 of a-z, 0-9 and -, unique across projects), `components` (`[{environment, name, response_time}]`; production is always first).",
    ),
    (
        "GET",
        "/api/projects/{project}/alerts",
        "read",
        "Alert settings (a webhook URL or ntfy topic that is set reads `(set)`).",
    ),
    (
        "PUT",
        "/api/projects/{project}/alerts",
        "write",
        "Change alert settings: the fields sent, over the stored ones; `(set)` keeps a stored webhook or topic.",
    ),
    (
        "POST",
        "/api/projects/{project}/alerts/test",
        "write",
        "Send a test message on every alert channel; `sent` lists each channel and why it failed.",
    ),
    (
        "PUT",
        "/api/projects/{project}/tickets/target",
        "write",
        "Set the GitHub repository tickets are filed in (`repo`, owner/name; empty unsets it). The token is set on the Tickets page.",
    ),
    (
        "GET",
        "/api/projects/{project}/churn/flags",
        "read",
        "Flags made from the users at risk, with how many treated and held-out users were active in the last 7 days.",
    ),
    (
        "POST",
        "/api/projects/{project}/churn/flags",
        "write",
        "Make a flag, off, listing the users at risk now with a control group (`holdout`, percent, default 20, at most 50).",
    ),
    (
        "GET",
        "/api/projects/{project}/acquisition",
        "read",
        "Users per channel, source, campaign or referrer (`by`), first or last touch (`touch`).",
    ),
    (
        "GET",
        "/api/projects/{project}/forms",
        "read",
        "Forms and where people give up.",
    ),
    (
        "GET",
        "/api/projects/{project}/audience",
        "read",
        "Devices, browsers, countries and languages.",
    ),
    (
        "GET",
        "/api/projects/{project}/live",
        "read",
        "Who is on the app now.",
    ),
    (
        "GET",
        "/api/projects/{project}/surveys",
        "read",
        "Micro-surveys.",
    ),
    (
        "PUT",
        "/api/projects/{project}/surveys/{key}",
        "write",
        "Create or change a survey.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/surveys/{key}",
        "write",
        "Delete a survey.",
    ),
    (
        "GET",
        "/api/projects/{project}/digest",
        "read",
        "The weekly digest.",
    ),
    (
        "GET",
        "/api/projects/{project}/replays",
        "read",
        "Recordings.",
    ),
    (
        "GET",
        "/api/projects/{project}/replays/{session}",
        "read",
        "One recording's chunks.",
    ),
    ("GET", "/api/audit", "admin", "The audit log."),
    (
        "GET",
        "/api/tokens",
        "admin",
        "API tokens (never their values).",
    ),
    (
        "POST",
        "/api/tokens",
        "admin",
        "Create a scoped API token; its value is shown once.",
    ),
    (
        "DELETE",
        "/api/tokens/{id}",
        "admin",
        "Revoke an API token.",
    ),
    (
        "POST",
        "/api/login-links",
        "admin",
        "A one-time login link or an invite.",
    ),
    ("GET", "/api/users", "admin", "Dashboard users."),
    (
        "PATCH",
        "/api/users/{id}",
        "admin",
        "Change a user's role, projects or zone.",
    ),
    ("DELETE", "/api/users/{id}", "admin", "Remove a user."),
    ("GET", "/api/openapi.json", "none", "This document."),
    (
        "GET",
        "/api/projects/{project}/feature-map/features/{key}",
        "read",
        "One feature: its context and numbers.",
    ),
    (
        "GET",
        "/api/projects/{project}/replays/{session}/comments",
        "read",
        "Comments on a recording.",
    ),
    (
        "POST",
        "/api/projects/{project}/replays/{session}/comments",
        "write",
        "Comment on a recording.",
    ),
    (
        "PUT",
        "/api/projects/{project}/sourcemaps/{release}/{bundle}",
        "write",
        "Upload a source map for a release.",
    ),
];

fn params(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|name| json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}}))
        .collect()
}

/// The OpenAPI 3.1 document of [`ROUTES`], relative to `/api/v1`.
pub(crate) fn document() -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for (method, path, scope, summary) in ROUTES {
        let rel = path.strip_prefix("/api").unwrap_or(path);
        let mut op = json!({
            "summary": summary,
            "x-scope": scope,
            "parameters": params(path),
            "responses": {
                "200": {"description": "OK", "content": {"application/json": {}}},
                "default": {"description": "An error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}},
            },
        });
        if matches!(*method, "POST" | "PUT" | "PATCH") {
            op["requestBody"] =
                json!({"content": {"application/json": {"schema": {"type": "object"}}}});
        }
        if *scope != "none" {
            op["security"] = json!([{"bearer": []}]);
        }
        let entry = paths.entry(rel.to_owned()).or_insert_with(|| json!({}));
        entry[method.to_ascii_lowercase()] = op;
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": "OpenLens API", "version": env!("CARGO_PKG_VERSION")},
        "servers": [{"url": "/api/v1"}, {"url": "/api", "description": "Unversioned alias"}],
        "paths": paths,
        "components": {
            "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}},
            "schemas": {"Error": {
                "type": "object",
                "required": ["code", "message"],
                "properties": {
                    "code": {"type": "string", "enum": crate::apierr::CODES},
                    "message": {"type": "string"},
                    "error": {"type": "string", "deprecated": true},
                },
            }},
        },
    })
}

/// `GET /api/openapi.json`.
pub(crate) async fn serve() -> Json<Value> {
    Json(document())
}
