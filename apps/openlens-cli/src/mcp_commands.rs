//! MCP tools over engine commands that only a page ran before (#525,
//! follow-up of #308, D-058): goals' removal and objectives, alerts and
//! uptime (and its history, #559), the GitHub repository of tickets, switching a flag, churn and
//! the win-back flag, and the explore reports. Like every tool, each one
//! calls the REST API (D-020). Kept apart from `mcp.rs` so that file stays
//! small; [`specs`] are listed after its tools and [`call`] answers the
//! names it does not know.

use openlens_mcp::ToolSpec;
use serde_json::{Value, json};

use crate::{Body, Client};

fn s(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::to_owned)
}

/// A path segment or query value, percent-encoded.
fn enc(v: &str) -> String {
    v.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// `/api/projects/{project}` and `tail`.
fn at(args: &Value, tail: &str) -> String {
    let p = s(args, "project").unwrap_or_else(|| "default".into());
    format!("/api/projects/{}{tail}", enc(&p))
}

/// `tail` with the string arguments `keys` that were given as its query.
fn query(args: &Value, tail: &str, keys: &[&str]) -> String {
    let q: Vec<String> = keys
        .iter()
        .filter_map(|k| s(args, k).map(|v| format!("{k}={}", enc(&v))))
        .collect();
    if q.is_empty() {
        at(args, tail)
    } else {
        at(args, &format!("{tail}?{}", q.join("&")))
    }
}

fn ok_or(status: u16, v: Value) -> Result<Value, String> {
    if (200..300).contains(&status) {
        Ok(v)
    } else {
        Err(crate::api_error(status, &v))
    }
}

fn get(c: &Client, path: &str) -> Result<Value, String> {
    let (st, v) = c.call("GET", path, None)?;
    ok_or(st, v)
}

fn send(c: &Client, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let (st, v) = c.call(method, path, body.map(Body::Json))?;
    ok_or(st, v)
}

const FILTERS: &[&str] = &["env", "role", "release"];

/// The alert settings an agent may change, as `PUT /alerts` takes them.
const ALERT_FIELDS: &[&str] = &[
    "down",
    "down_wait",
    "traffic",
    "regressions",
    "cert",
    "failing",
    "budget_spent",
    "budget_burn",
    "quiet",
    "health_path",
    "email",
    "webhook",
    "ntfy",
];

/// The tools of this module.
pub(crate) fn specs() -> Vec<ToolSpec> {
    let proj = || json!({ "project": { "type": "string", "description": "Project id (default: default)" } });
    let with = |extra: Value| {
        let mut p = proj();
        if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        p
    };
    let filters =
        || json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"}});
    let filtered = |extra: Value| {
        let mut f = filters();
        if let (Some(a), Some(b)) = (f.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        with(f)
    };
    vec![
        ToolSpec {
            name: "routes",
            description: "The app's server routes over the last `days` (1 to 90, default 7) in one environment (production unless env), from the server SDK's middleware: per method and route template, requests, 5xx count and share, p50 and p95 in ms (upper bounds of the duration histogram), the largest duration, and the feature whose key action the route is (`server:` in the feature map). Release verdicts judge the same routes (5xx share up significantly, or p95 1.5 times and 200 ms slower); `release_report` shows them as `route` checks. Empty, with a note, until the server SDK sends stats.",
            input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"days":{"type":"integer","minimum":1,"maximum":90}}))}),
        },
        ToolSpec {
            name: "delete_goal",
            description: "Remove a goal (its id from `goals`). Objectives that used it as a key result keep the others.",
            input_schema: json!({"type":"object","properties":with(json!({"goal":{"type":"string"}})),"required":["goal"]}),
        },
        ToolSpec {
            name: "set_objective",
            description: "Set an objective: a sentence (`title`), one to three existing goal ids from `goals` as its key results, and the last day of its period (`end`, YYYY-MM-DD, within a year). At most 10 open objectives per project.",
            input_schema: json!({"type":"object","properties":with(json!({
                "title":{"type":"string"},
                "goals":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":3},
                "end":{"type":"string"}
            })),"required":["title","goals","end"]}),
        },
        ToolSpec {
            name: "delete_objective",
            description: "Remove an objective (its id from `goals`); its goals stay.",
            input_schema: json!({"type":"object","properties":with(json!({"objective":{"type":"string"}})),"required":["objective"]}),
        },
        ToolSpec {
            name: "status",
            description: "Is the app up now: up, down, maintenance (a planned window covers production), unknown or off, since when, the open incidents and the maintenance windows running or starting within 7 days. Check it around a deploy.",
            input_schema: json!({"type":"object","properties":proj()}),
        },
        ToolSpec {
            name: "alerts",
            description: "Uptime and alerting: the status, availability over a day and a week (planned maintenance left out, `maintenance_left_out` in seconds), the maintenance windows, response times over a day (median, p95), incidents of 30 days, the last 15 checks, and the alert settings (what raises an alert, quiet hours, the probe's path, channels; a webhook or ntfy topic that is set reads '(set)').",
            input_schema: json!({"type":"object","properties":proj()}),
        },
        ToolSpec {
            name: "uptime_days",
            description: "Uptime history, one row per UTC day: from and to (YYYY-MM-DD, at most 400 days; default the last 90), env (default production). Per day: state (up, partial: down under an hour, down: an hour or more, none: not checked), availability (share of good checks), up_seconds, and stats (checks, failed, down_seconds, measured_seconds, incidents, p50/p95/max ms); plus the range's availability, seconds up and measured. Read it for an SLO or a monthly report.",
            input_schema: json!({"type":"object","properties":with(json!({
                "from":{"type":"string"},
                "to":{"type":"string"},
                "env":{"type":"string"}
            }))}),
        },
        ToolSpec {
            name: "availability",
            description: "Availability of an environment (env, default production) over 24h, 7d, 30d, 90d, this month, last month (project's zone) and 365d: time up over time measured, seconds not measured (no check: neither up nor down), seconds of planned maintenance left out, incidents, p95 ms; and slo when a target is set: target, 30-day availability, error budget, spent and left (seconds), burn rate over the last hour and day (1 = the budget lasts 30 days). The figures of an SLA or a monthly report.",
            input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"}}))}),
        },
        ToolSpec {
            name: "availability_target",
            description: "Set or remove an environment's availability target over a rolling 30 days: target a share from 0.5 to 0.99999 (0.999 for 99.9%), or null to remove it; env defaults to production. Alerts go out when its error budget is spent or burning fast.",
            input_schema: json!({"type":"object","properties":with(json!({
                "env":{"type":"string"},
                "target":{"type":["number","null"],"minimum":0.5,"maximum":0.99999}
            })),"required":["target"]}),
        },
        ToolSpec {
            name: "incidents",
            description: "Every incident of the project (kept as long as the project), newest first, page by page: limit (1-100, default 50) and before (the previous page's next). Each has its id, rule (down, traffic, failing, cert, insight:…, error:…, env:<env>:down…), start, end (null while open), minutes and cause.",
            input_schema: json!({"type":"object","properties":with(json!({
                "before":{"type":"integer"},
                "limit":{"type":"integer","minimum":1,"maximum":100}
            }))}),
        },
        ToolSpec {
            name: "alert_set",
            description: "Change alert settings: only the fields given change. down (probe the app address every minute), down_wait (minutes before announcing), traffic (traffic stopped), regressions (regressions and new error groups), cert (certificate expiring), failing (spike of failing requests), budget_spent and budget_burn (an availability target's error budget spent, or gone within 2 days at the last day's rate), quiet ([start, end) hours in the project's zone, or null), health_path, email, webhook (Slack, Discord or JSON URL), ntfy (topic or URL). With test true, then sends a test message on every channel and says which failed.",
            input_schema: json!({"type":"object","properties":with(json!({
                "down":{"type":"boolean"},
                "down_wait":{"type":"integer","minimum":0},
                "traffic":{"type":"boolean"},
                "regressions":{"type":"boolean"},
                "cert":{"type":"boolean"},
                "failing":{"type":"boolean"},
                "budget_spent":{"type":"boolean"},
                "budget_burn":{"type":"boolean"},
                "quiet":{"type":["array","null"],"items":{"type":"integer","minimum":0,"maximum":23},"minItems":2,"maxItems":2},
                "health_path":{"type":"string"},
                "email":{"type":"boolean"},
                "webhook":{"type":"string"},
                "ntfy":{"type":"string"},
                "test":{"type":"boolean","default":false}
            }))}),
        },
        ToolSpec {
            name: "ticket_target",
            description: "Set the GitHub repository (owner/name) that create_ticket with to='github' files issues in; empty unsets it. The GitHub token is set by a person on the Tickets page.",
            input_schema: json!({"type":"object","properties":with(json!({"repo":{"type":"string"}})),"required":["repo"]}),
        },
        ToolSpec {
            name: "flag_update",
            description: "Switch an existing flag on or off and/or set its rollout (0-100 %), keeping its targeting. Widen a rollout step by step once the board shows its cohort is fine; flag_set creates or rewrites a flag.",
            input_schema: json!({"type":"object","properties":with(json!({
                "key":{"type":"string"},
                "enabled":{"type":"boolean"},
                "rollout":{"type":"integer","minimum":0,"maximum":100}
            })),"required":["key"]}),
        },
        ToolSpec {
            name: "churn",
            description: "Who leaves and why (report): churn rate, net change, churned per week, survival and median lifetime, the last feature or page before leaving against where users who stayed were, errors on the last day, who is at risk now and a suggested churn_after_days; and the win-back flags made from the users at risk, with how many treated and held-out users were active in the last 7 days (winback_flags).",
            input_schema: json!({"type":"object","properties":proj()}),
        },
        ToolSpec {
            name: "winback_flag",
            description: "Make a flag, off, listing the users at risk now with a control group held out (holdout %, default 20, at most 50), for a win-back nudge behind openlens.isEnabled(key). Switch it on with flag_update; churn then compares the two groups.",
            input_schema: json!({"type":"object","properties":with(json!({"holdout":{"type":"integer","minimum":0,"maximum":50}}))}),
        },
        ToolSpec {
            name: "acquisition",
            description: "Where users come from: per channel (default), source, campaign or referrer (`by`), by first touch (default) or last touch (`touch`): users, those who reached a success event, those active on two days or more, and those who churned.",
            input_schema: json!({"type":"object","properties":filtered(json!({
                "by":{"type":"string","enum":["channel","source","campaign","referrer"]},
                "touch":{"type":"string","enum":["first","last"]}
            }))}),
        },
        ToolSpec {
            name: "forms",
            description: "The app's forms, the most abandoned first: sessions that started and submitted each, median time to fill, and per field the sessions, the abandons where it was the last field touched, corrections and median time; a session that gave up, to replay.",
            input_schema: json!({"type":"object","properties":filtered(json!({}))}),
        },
        ToolSpec {
            name: "audience",
            description: "Who uses the app, from page views of 30 days: users per country, device class, browser, OS, language and screen width. `geo` false means countries cannot be known on this server.",
            input_schema: json!({"type":"object","properties":filtered(json!({}))}),
        },
        ToolSpec {
            name: "live",
            description: "Who is on the app right now: users of the last 5 and 30 minutes, errors, roles, releases, features and pages of the last 30 minutes, and the latest events.",
            input_schema: json!({"type":"object","properties":filtered(json!({}))}),
        },
        ToolSpec {
            name: "flows",
            description: "How sessions move between features, step by step: the nodes of each step and the links between them with their sessions, and how many sessions reached a success event. `from` (a feature key) starts at its first use; `to: success` keeps the sessions that reached one.",
            input_schema: json!({"type":"object","properties":filtered(json!({
                "from":{"type":"string"},
                "to":{"type":"string","enum":["success"]}
            }))}),
        },
    ]
}

fn required(args: &Value, k: &str) -> Result<String, String> {
    s(args, k).ok_or_else(|| format!("{k} is required"))
}

fn set_objective(c: &Client, args: &Value) -> Result<Value, String> {
    let body = json!({
        "title": required(args, "title")?,
        "goals": args.get("goals").cloned().unwrap_or_else(|| json!([])),
        "end": required(args, "end")?,
    });
    send(c, "POST", &at(args, "/objectives"), Some(body))
}

fn alerts(c: &Client, args: &Value) -> Result<Value, String> {
    let uptime = get(c, &at(args, "/uptime"))?;
    let settings = get(c, &at(args, "/alerts"))?;
    Ok(json!({ "uptime": uptime, "settings": settings }))
}

fn alert_set(c: &Client, args: &Value) -> Result<Value, String> {
    let change: serde_json::Map<String, Value> = ALERT_FIELDS
        .iter()
        .filter_map(|k| args.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    let test = args.get("test").and_then(Value::as_bool).unwrap_or(false);
    if change.is_empty() && !test {
        return Err("give a setting to change, or test: true".into());
    }
    let changed = if change.is_empty() {
        Value::Null
    } else {
        send(c, "PUT", &at(args, "/alerts"), Some(Value::Object(change)))?
    };
    let tested = if test {
        send(c, "POST", &at(args, "/alerts/test"), None)?
    } else {
        Value::Null
    };
    Ok(json!({ "changed": changed, "test": tested }))
}

fn flag_update(c: &Client, args: &Value) -> Result<Value, String> {
    let key = required(args, "key")?;
    let mut body = serde_json::Map::new();
    if let Some(e) = args.get("enabled").and_then(Value::as_bool) {
        body.insert("enabled".into(), json!(e));
    }
    if let Some(r) = args.get("rollout").and_then(Value::as_u64) {
        body.insert("rollout".into(), json!(r.min(100)));
    }
    if body.is_empty() {
        return Err("give enabled, rollout or both".into());
    }
    let path = at(args, &format!("/flags/{}", enc(&key)));
    send(c, "PATCH", &path, Some(Value::Object(body)))
}

fn churn(c: &Client, args: &Value) -> Result<Value, String> {
    let report = get(c, &at(args, "/churn"))?;
    let flags = get(c, &at(args, "/churn/flags"))?;
    Ok(json!({ "report": report, "winback_flags": flags["flags"] }))
}

/// Run the tool `name`, or `None` when it is not one of [`specs`].
pub(crate) fn call(c: &Client, name: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "routes" => {
            let mut path = query(args, "/routes", &["env"]);
            if let Some(d) = args.get("days").and_then(Value::as_i64) {
                path.push(if path.contains('?') { '&' } else { '?' });
                path.push_str(&format!("days={d}"));
            }
            get(c, &path)
        }
        "delete_goal" => required(args, "goal").and_then(|g| {
            let path = at(args, &format!("/goals/{}", enc(&g)));
            send(c, "DELETE", &path, None)
        }),
        "set_objective" => set_objective(c, args),
        "delete_objective" => required(args, "objective").and_then(|o| {
            let path = at(args, &format!("/objectives/{}", enc(&o)));
            send(c, "DELETE", &path, None)
        }),
        "status" => get(c, &at(args, "/status")),
        "alerts" => alerts(c, args),
        "uptime_days" => get(c, &query(args, "/uptime/days", &["from", "to", "env"])),
        "availability" => get(c, &query(args, "/uptime/report", &["env"])),
        "availability_target" if args.get("target").is_none() => {
            Err("target is required: a share such as 0.999, or null to remove it".into())
        }
        "availability_target" => {
            let body = json!({
                "environment": s(args, "env"),
                "target": args.get("target").and_then(Value::as_f64),
            });
            send(c, "PUT", &at(args, "/uptime/target"), Some(body))
        }
        "incidents" => {
            let mut q: Vec<String> = Vec::new();
            for k in ["before", "limit"] {
                if let Some(n) = args.get(k).and_then(Value::as_i64) {
                    q.push(format!("{k}={n}"));
                }
            }
            get(c, &at(args, &format!("/incidents?{}", q.join("&"))))
        }
        "alert_set" => alert_set(c, args),
        "ticket_target" => required(args, "repo").and_then(|repo| {
            let body = json!({ "repo": repo });
            send(c, "PUT", &at(args, "/tickets/target"), Some(body))
        }),
        "flag_update" => flag_update(c, args),
        "churn" => churn(c, args),
        "winback_flag" => {
            let holdout = args
                .get("holdout")
                .and_then(Value::as_u64)
                .map(|h| h.min(50));
            let body = json!({ "holdout": holdout });
            send(c, "POST", &at(args, "/churn/flags"), Some(body))
        }
        "acquisition" => {
            let keys = ["env", "role", "release", "by", "touch"];
            get(c, &query(args, "/acquisition", &keys))
        }
        "forms" => get(c, &query(args, "/forms", FILTERS)),
        "audience" => get(c, &query(args, "/audience", FILTERS)),
        "live" => get(c, &query(args, "/live", FILTERS)),
        "flows" => {
            let keys = ["env", "role", "release", "from", "to"];
            get(c, &query(args, "/flows", &keys))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_tool_is_answered_and_others_are_not() {
        let client = Client::new(Some("http://localhost:1".into()), Some("t".into())).unwrap();
        // Enough arguments for each tool to reach the server (which is not
        // there): a tool this module does not answer would return `None`.
        let args = json!({"goal": "g", "title": "t", "end": "2027-01-01", "objective": "o",
            "repo": "a/b", "key": "k", "test": true});
        for t in specs() {
            assert!(
                call(&client, t.name, &args).is_some(),
                "tool `{}` is listed but not answered",
                t.name
            );
        }
        assert!(call(&client, "goals", &args).is_none());
    }

    #[test]
    fn query_values_and_path_segments_cannot_add_parameters() {
        let args = json!({"project": "a/b", "env": "prod&role=x", "by": "source"});
        let path = query(&args, "/acquisition", &["env", "by"]);
        assert!(
            path.starts_with("/api/projects/a%2Fb/acquisition?"),
            "{path}"
        );
        assert!(path.contains("env=prod%26role%3Dx"), "{path}");
        assert!(path.ends_with("&by=source"), "{path}");
    }
}
