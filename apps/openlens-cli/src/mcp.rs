//! OpenLens tools for the MCP server (`openlens-cli mcp`).
//!
//! Every tool goes through the same HTTP API as the CLI. Tools that read
//! files only accept paths inside `--automation-read-root`.

use std::path::{Path, PathBuf};

use openlens_mcp::{ToolSpec, Tools};
use serde_json::{Value, json};

use crate::{Body, Client};

pub(crate) struct OpenLensTools {
    client: Client,
    read_root: PathBuf,
    // Reserved for tools that write (exports); kept so the root is enforced
    // from day one.
    _write_root: PathBuf,
}

impl OpenLensTools {
    pub(crate) fn new(client: Client, read: &Path, write: &Path) -> Result<Self, String> {
        let canon = |p: &Path| {
            p.canonicalize()
                .map_err(|e| format!("{}: {e}", p.display()))
        };
        let read_root = canon(read)?;
        let write_root = canon(write)?;
        for r in [&read_root, &write_root] {
            if !r.is_dir() {
                return Err(format!("{} is not a directory", r.display()));
            }
        }
        Ok(Self {
            client,
            read_root,
            _write_root: write_root,
        })
    }

    /// Resolve `path` (relative to the read root) and refuse anything outside it.
    fn readable(&self, path: &str) -> Result<PathBuf, String> {
        let p = self.read_root.join(path);
        let canon = p.canonicalize().map_err(|e| format!("{path}: {e}"))?;
        if canon.starts_with(&self.read_root) {
            Ok(canon)
        } else {
            Err(format!("{path} is outside the read root"))
        }
    }
}

fn s(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn project(args: &Value) -> String {
    s(args, "project").unwrap_or_else(|| "default".into())
}

fn ok_or(status: u16, v: Value) -> Result<Value, String> {
    if (200..300).contains(&status) {
        Ok(v)
    } else {
        Err(format!("HTTP {status}: {v}"))
    }
}

impl Tools for OpenLensTools {
    fn list(&self) -> Vec<ToolSpec> {
        let proj = json!({ "project": { "type": "string", "description": "Project id (default: default)" } });
        let with = |extra: Value| {
            let mut p = proj.clone();
            if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
                a.extend(b.clone());
            }
            p
        };
        vec![
            ToolSpec {
                name: "guide",
                description: "The wiring guide: how to connect an app to OpenLens so the PM's first login shows a populated board. Read this first.",
                input_schema: json!({"type":"object","properties":{}}),
            },
            ToolSpec {
                name: "projects",
                description: "List projects with their capture keys.",
                input_schema: json!({"type":"object","properties":{}}),
            },
            ToolSpec {
                name: "init",
                description: "Create a project; returns the capture key and the snippet to add to the app. Pass the team's IANA time zone (e.g. Europe/Paris): days and weeks of every KPI are counted in it.",
                input_schema: json!({"type":"object","properties":{"name":{"type":"string"},"timezone":{"type":"string"}},"required":["name"]}),
            },
            ToolSpec {
                name: "set_timezone",
                description: "Set a project's time zone (IANA name, e.g. Europe/Paris): where its days and weeks start.",
                input_schema: json!({"type":"object","properties":{"project":{"type":"string"},"timezone":{"type":"string"}},"required":["project","timezone"]}),
            },
            ToolSpec {
                name: "feature_map_lint",
                description: "Validate an openlens.features.yaml (path relative to the read root) and check it against the app code under `src` (default: the read root).",
                input_schema: json!({"type":"object","properties":{"file":{"type":"string"},"src":{"type":"string"}},"required":["file"]}),
            },
            ToolSpec {
                name: "feature_map_push",
                description: "Upload openlens.features.yaml (path relative to the read root) as the project's new feature map. Returns issues with fixes when invalid.",
                input_schema: json!({"type":"object","properties":with(json!({"file":{"type":"string"}})),"required":["file"]}),
            },
            ToolSpec {
                name: "map_health",
                description: "How good the pushed feature map is, measured on 30 days of usage: a score out of 100, findings ranked by impact (pages no feature claims, overlapping routes, dead features, success events that never fire or fire on every use, data-feature values not declared, requires contradicted by usage), each with evidence and a YAML fix.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "map_overview",
                description: "Every feature with its area, status and numbers (uses, users, users who reached success; 30 days vs the 30 before), the roll-up by area and the adoption ladder. Start here, then feature_context for one feature.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "feature_context",
                description: "One feature in depth: its definition (area, requires, renamed_from), health and findings, users and success by role, top pages, the ladder rungs it is on, features that require it and its area peers.",
                input_schema: json!({"type":"object","properties":with(json!({"key":{"type":"string"}})),"required":["key"]}),
            },
            ToolSpec {
                name: "suggest_map_changes",
                description: "Changes to make to openlens.features.yaml, ranked by impact, each with its evidence and a YAML patch. Nothing is applied: edit the file, then feature_map_push.",
                input_schema: json!({"type":"object","properties":with(json!({"limit":{"type":"integer","minimum":1,"maximum":50}}))}),
            },
            ToolSpec {
                name: "goals",
                description: "Goals on KPIs and objectives: value now, target, deadline, status (reached, on track, at risk, off track), guardrails and whether they are breached, what moved each goal (releases with their effect, flag/survey/map changes and notes since it was set), and suggested targets per KPI.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "set_goal",
                description: "Set a goal: a KPI key from `goals` suggestions (or success:<feature>), a target in KPI units (a share as 0-1), and a deadline (YYYY-MM-DD) or a number of releases. Optional guardrails: KPIs that must not get worse, with a limit (default 5% worse than now).",
                input_schema: json!({"type":"object","properties":with(json!({
                    "kpi":{"type":"string"},
                    "target":{"type":"number"},
                    "deadline":{"type":"string"},
                    "releases":{"type":"integer","minimum":1,"maximum":50},
                    "owner":{"type":"string"},
                    "guardrails":{"type":"array","items":{"type":"object","properties":{"kpi":{"type":"string"},"limit":{"type":"number"}},"required":["kpi"]}}
                })),"required":["kpi","target"]}),
            },
            ToolSpec {
                name: "create_ticket",
                description: "Turn a finding into a ticket for the app's backlog: kind insight (subject '<card kind>:<card subject>'), error (subject = the error group fingerprint), goal (subject = goal id) or map (subject '<finding kind>:<feature>'). Returns the prefilled Markdown (problem, evidence, hypothesis, success metric, acceptance criteria); with to='github', opens the GitHub issue, or updates the one already open for that finding. OpenLens then measures the metric 14 days after the release that ships it and comments on the issue.",
                input_schema: json!({"type":"object","properties":with(json!({"kind":{"type":"string","enum":["insight","error","goal","map"]},"subject":{"type":"string"},"to":{"type":"string","enum":["markdown","github"]}})),"required":["kind","subject"]}),
            },
            ToolSpec {
                name: "tickets",
                description: "Tickets made from findings, the release that shipped each, and the measured effect (improved / no effect / worse): the learning log.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "verify",
                description: "Wiring checklist: events arrive, users identified with a role, release tagged, map pushed, events per feature. Each failing check carries its fix.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "insights",
                description: "What to focus on: the ranked insight cards (evidence, cohort size, next step), paths to value, readiness and role gaps.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "digest",
                description: "This week vs last week (active users, sessions, errors, completed tasks per feature) plus the top 3 insight cards, as JSON and ready-to-send Markdown.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "kpi_report",
                description: "Headline KPIs in one compact payload: users (DAU/WAU/MAU, stickiness), sessions, vitals, top features and roles, latest two releases. With `by` (role|account|plan|page|feature), also users, sessions, events, tasks and errors per value (30 days). Roles are 'role at event time'.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"},"by":{"type":"string","enum":["role","account","plan","page","feature"]}}))}),
            },
            ToolSpec {
                name: "user_timeline",
                description: "One user's story: role, account, plan, role changes, recorded sessions (with dashboard links) and their latest 100 events (90 days). Use it to see what a churned or frustrated user did.",
                input_schema: json!({"type":"object","properties":with(json!({"distinct_id":{"type":"string"}})),"required":["distinct_id"]}),
            },
            ToolSpec {
                name: "board",
                description: "The Focus board numbers (KPIs by role/environment/release, features, roles, role gaps, releases, vitals).",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "errors",
                description: "Top error groups (normalised message + source), with users, sessions, roles, releases and places hit. Last 30 days.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "sql",
                description: "Read-only SQL (DataFusion) on table `events`: uuid, event, distinct_id, session_id, timestamp, url, path, feature, role, account, plan, environment, release, properties (JSON text), date. Prune with `date >= 'YYYY-MM-DD'`. Max 10k rows.",
                input_schema: json!({"type":"object","properties":with(json!({"query":{"type":"string"}})),"required":["query"]}),
            },
            ToolSpec {
                name: "flags",
                description: "The project's feature flags (key, enabled, rollout %, target roles and accounts, property rules, variants).",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "flag_set",
                description: "Create or update a flag: put a risky change behind openlens.isEnabled(key), start with a small rollout or one role, then widen it once the board shows its cohort is fine.",
                input_schema: json!({"type":"object","properties":with(json!({
                    "key":{"type":"string"},
                    "enabled":{"type":"boolean"},
                    "rollout":{"type":"integer","minimum":0,"maximum":100},
                    "roles":{"type":"array","items":{"type":"string"}},
                    "accounts":{"type":"array","items":{"type":"string"}},
                    "description":{"type":"string"},
                    "rules":{"type":"array","items":{"type":"string"},"description":"Property rules, all must match: 'plan in pro, team', 'country not in FR' (any identify() property; a user without it is out)"}
                })),"required":["key"]}),
            },
            ToolSpec {
                name: "surveys",
                description: "The project's micro-surveys and their results (answers, mean rating, by role, latest text answers, dismissals), last 30 days.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "survey_set",
                description: "Create or update a micro-survey: one question shown right after `trigger` (a success event like entry_saved) to `sample`% of users (optionally some roles), once each. kind: rating (1-5), yes_no, choice (2-6 choices) or text. Ask about the moment that just happened, in plain words.",
                input_schema: json!({"type":"object","properties":with(json!({
                    "key":{"type":"string"},
                    "question":{"type":"string"},
                    "kind":{"type":"string","enum":["rating","yes_no","choice","text"]},
                    "choices":{"type":"array","items":{"type":"string"}},
                    "trigger":{"type":"string"},
                    "sample":{"type":"integer","minimum":0,"maximum":100},
                    "roles":{"type":"array","items":{"type":"string"}},
                    "enabled":{"type":"boolean"}
                })),"required":["key","question","kind","trigger"]}),
            },
            ToolSpec {
                name: "login_link",
                description: "A one-time login link (7 days). Without email: an owner link for the PM. With email (+ role viewer|admin, optional projects): invites that person with that access.",
                input_schema: json!({"type":"object","properties":{"email":{"type":"string"},"role":{"type":"string","enum":["viewer","admin"]},"projects":{"type":"array","items":{"type":"string"}}}}),
            },
        ]
    }

    fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        let c = &self.client;
        match name {
            "guide" => {
                let (st, v) = c.call("GET", "/agents.md", None)?;
                ok_or(st, json!({ "guide": v }))
            }
            "projects" => {
                let (st, v) = c.call("GET", "/api/projects", None)?;
                ok_or(st, v)
            }
            "init" => {
                let name = s(args, "name").ok_or("name is required")?;
                let mut body = json!({ "name": name });
                if let Some(tz) = s(args, "timezone") {
                    body["timezone"] = json!(tz);
                }
                let (st, v) = c.call("POST", "/api/projects", Some(Body::Json(body)))?;
                ok_or(st, v)
            }
            "set_timezone" => {
                let project = s(args, "project").ok_or("project is required")?;
                let tz = s(args, "timezone").ok_or("timezone is required")?;
                let (st, v) = c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(json!({ "timezone": tz }))),
                )?;
                ok_or(st, v)
            }
            "feature_map_lint" => {
                let file = self.readable(&s(args, "file").ok_or("file is required")?)?;
                let src = match s(args, "src") {
                    Some(p) => self.readable(&p)?,
                    None => self.read_root.clone(),
                };
                let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
                Ok(crate::lint::lint(&text, &src))
            }
            "feature_map_push" => {
                let file = self.readable(&s(args, "file").ok_or("file is required")?)?;
                let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
                let (st, v) = c.call(
                    "PUT",
                    &format!("/api/projects/{}/feature-map", project(args)),
                    Some(Body::Text(text)),
                )?;
                ok_or(st, v)
            }
            "map_health" | "map_overview" | "suggest_map_changes" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/feature-map/health", project(args)),
                    None,
                )?;
                let v = ok_or(st, v)?;
                Ok(match name {
                    "map_overview" => json!({
                        "score": v["score"],
                        "coverage": v["coverage"],
                        "features": v["features"],
                        "areas": v["areas"],
                        "ladder": v["ladder"],
                    }),
                    "suggest_map_changes" => {
                        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10);
                        let limit = usize::try_from(limit.clamp(1, 50)).unwrap_or(10);
                        let picks: Vec<Value> = v["findings"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter(|f| !f["fix"].is_null())
                                    .take(limit)
                                    .map(|f| json!({ "change": f["message"], "evidence": f["evidence"], "patch": f["fix"], "feature": f["feature"], "impact": f["impact"] }))
                                    .collect()
                            })
                            .unwrap_or_default();
                        json!({ "score": v["score"], "suggestions": picks, "apply": "edit openlens.features.yaml, then feature_map_push" })
                    }
                    _ => v,
                })
            }
            "feature_context" => {
                let key = s(args, "key").ok_or("key is required")?;
                if !openlens_featuremap::valid_key(&key) {
                    return Err(format!("`{key}` is not a feature key"));
                }
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/feature-map/features/{key}", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "goals" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/goals", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "set_goal" => {
                let mut body = args.clone();
                if let Some(o) = body.as_object_mut() {
                    o.remove("project");
                }
                let (st, v) = c.call(
                    "POST",
                    &format!("/api/projects/{}/goals", project(args)),
                    Some(Body::Json(body)),
                )?;
                ok_or(st, v)
            }
            "create_ticket" => {
                let mut body = args.clone();
                if let Some(o) = body.as_object_mut() {
                    o.remove("project");
                }
                let (st, v) = c.call(
                    "POST",
                    &format!("/api/projects/{}/tickets", project(args)),
                    Some(Body::Json(body)),
                )?;
                ok_or(st, v)
            }
            "tickets" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/tickets", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "verify" => crate::verify_report(c, &project(args)),
            "insights" | "kpi_report" | "board" => {
                let mut q = Vec::new();
                for k in ["env", "role", "release"] {
                    if let Some(v) = s(args, k) {
                        q.push(format!("{k}={}", v.replace(['&', '#', '?', ' '], "")));
                    }
                }
                let path = format!("/api/projects/{}/board?{}", project(args), q.join("&"));
                let (st, v) = c.call("GET", &path, None)?;
                let v = ok_or(st, v)?;
                Ok(match name {
                    "insights" => json!({
                        "insights": v["insights"],
                        "paths_to_value": v["journeys"],
                        "readiness": v["readiness"],
                        "role_gaps": v["role_gaps"],
                        "feature_map_version": v["map_version"],
                    }),
                    "kpi_report" => {
                        let top = |k: &str, n: usize| {
                            json!(
                                v[k].as_array().map(|a| a
                                    .iter()
                                    .take(n)
                                    .cloned()
                                    .collect::<Vec<_>>())
                            )
                        };
                        let breakdown = match s(args, "by") {
                            Some(by) => {
                                let mut bq = q.clone();
                                bq.push(format!("by={}", by.replace(['&', '#', '?', ' '], "")));
                                let path = format!(
                                    "/api/projects/{}/breakdown?{}",
                                    project(args),
                                    bq.join("&")
                                );
                                let (st, b) = c.call("GET", &path, None)?;
                                ok_or(st, b)?
                            }
                            None => Value::Null,
                        };
                        json!({
                            "role_rule": "role at event time",
                            "breakdown": breakdown,
                            "filters": v["filters"],
                            "users": v["users"],
                            "sessions": v["sessions"],
                            "vitals": v["vitals"],
                            "features": top("features", 8),
                            "roles": top("roles", 8),
                            "releases": top("releases", 2),
                            "total_events": v["total_events"],
                        })
                    }
                    _ => v,
                })
            }
            "digest" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/digest", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "user_timeline" => {
                let who = s(args, "distinct_id").ok_or("distinct_id is required")?;
                let who: String = who
                    .bytes()
                    .map(|b| {
                        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                            (b as char).to_string()
                        } else {
                            format!("%{b:02X}")
                        }
                    })
                    .collect();
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/people/{who}", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "errors" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/errors", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "sql" => {
                let query = s(args, "query").ok_or("query is required")?;
                Ok(json!({ "rows": c.sql(&project(args), &query)? }))
            }
            "flags" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/flags", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "flag_set" => {
                let key = s(args, "key").ok_or("key is required")?;
                let body = json!({
                    "key": key,
                    "enabled": args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    "rollout": args.get("rollout").and_then(Value::as_u64).unwrap_or(0),
                    "roles": args.get("roles").cloned().unwrap_or_else(|| json!([])),
                    "accounts": args.get("accounts").cloned().unwrap_or_else(|| json!([])),
                    "description": s(args, "description").unwrap_or_default(),
                    "rules": args
                        .get("rules")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(openlens_flags::Rule::parse)
                                .collect::<Result<Vec<_>, String>>()
                        })
                        .transpose()?
                        .unwrap_or_default(),
                });
                let (st, v) = c.call(
                    "PUT",
                    &format!("/api/projects/{}/flags/{key}", project(args)),
                    Some(Body::Json(body)),
                )?;
                ok_or(st, v)
            }
            "surveys" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/surveys", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "survey_set" => {
                let key = s(args, "key").ok_or("key is required")?;
                let body = json!({
                    "key": key,
                    "question": s(args, "question").ok_or("question is required")?,
                    "kind": s(args, "kind").ok_or("kind is required")?,
                    "choices": args.get("choices").cloned().unwrap_or_else(|| json!([])),
                    "trigger": s(args, "trigger").ok_or("trigger is required")?,
                    "sample": args.get("sample").and_then(Value::as_u64).unwrap_or(10),
                    "roles": args.get("roles").cloned().unwrap_or_else(|| json!([])),
                    "enabled": args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                });
                let (st, v) = c.call(
                    "PUT",
                    &format!("/api/projects/{}/surveys/{key}", project(args)),
                    Some(Body::Json(body)),
                )?;
                ok_or(st, v)
            }
            "login_link" => {
                let body = s(args, "email").map(|e| {
                    Body::Json(json!({
                        "email": e,
                        "role": s(args, "role").unwrap_or_else(|| "viewer".into()),
                        "projects": args.get("projects").cloned(),
                    }))
                });
                let (st, v) = c.call("POST", "/api/login-links", body)?;
                ok_or(st, v)
            }
            _ => Err(format!("unknown tool {name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_call_handles_is_listed_for_agents() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new(Some("http://localhost:1".into()), Some("t".into())).unwrap();
        let tools = OpenLensTools::new(client, dir.path(), dir.path()).unwrap();
        let listed: Vec<&str> = tools.list().iter().map(|t| t.name).collect();
        // Parse the dispatch arms of `call` from this file, so a new arm
        // without a ToolSpec fails here.
        let src = include_str!("mcp.rs");
        let body = &src[src.find("fn call(&self").unwrap()..src.find("#[cfg(test)]").unwrap()];
        let mut checked = 0;
        for line in body.lines().map(str::trim) {
            if !line.ends_with("=> {") && !line.contains("\" =>") {
                continue;
            }
            let arm = line.split("=>").next().unwrap_or("");
            for name in arm.split('|').map(|n| n.trim().trim_matches('"')) {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    assert!(
                        listed.contains(&name),
                        "tool `{name}` is handled but not listed"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked >= 12, "parsed only {checked} dispatch names");
    }

    #[test]
    fn files_outside_the_read_root_are_refused_even_when_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("ok.yaml"), "x").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), root.join("link.txt")).unwrap();
        let client = Client::new(Some("http://localhost:1".into()), Some("t".into())).unwrap();
        let tools = OpenLensTools::new(client, &root, &root).unwrap();
        assert!(tools.readable("ok.yaml").is_ok());
        assert!(tools.readable("../secret.txt").is_err());
        #[cfg(unix)]
        assert!(
            tools.readable("link.txt").is_err(),
            "a symlink cannot point out of the root"
        );
    }
}
