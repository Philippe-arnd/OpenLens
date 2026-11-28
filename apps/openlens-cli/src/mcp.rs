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

/// The `PATCH /api/projects/{id}` body of the `environments` tool's
/// optional changes, or `None` when it only reads. Each change needs its
/// companion argument (`host`+`env`, `alias`+`to`, `monitor`+`url`).
fn environment_change(args: &Value) -> Result<Option<Value>, String> {
    let mut body = serde_json::Map::new();
    match (s(args, "host"), s(args, "env")) {
        (Some(host), Some(env)) => {
            body.insert(
                "environment_rule".into(),
                json!({ "host": host, "env": env }),
            );
        }
        (None, None) => {}
        _ => return Err("send host and env together to set a rule".into()),
    }
    match (s(args, "alias"), s(args, "to")) {
        (Some(from), Some(to)) => {
            body.insert(
                "environment_alias".into(),
                json!({ "from": from, "to": to }),
            );
        }
        (None, None) => {}
        _ => return Err("send alias and to together to set an alias".into()),
    }
    match (s(args, "monitor"), s(args, "url")) {
        (Some(env), Some(url)) => {
            let level = s(args, "level").unwrap_or_default();
            body.insert(
                "environment_monitor".into(),
                json!({ "env": env, "url": url, "level": level }),
            );
        }
        (None, None) => {}
        _ => return Err("send monitor and url together to check an environment".into()),
    }
    for (arg, field) in [
        ("remove", "remove_environment_rule"),
        ("unalias", "remove_environment_alias"),
        ("hide", "hide_environment"),
        ("show", "show_environment"),
        ("unmonitor", "remove_environment_monitor"),
    ] {
        if let Some(v) = s(args, arg) {
            body.insert(field.into(), json!(v));
        }
    }
    Ok((!body.is_empty()).then_some(Value::Object(body)))
}

fn ok_or(status: u16, v: Value) -> Result<Value, String> {
    if (200..300).contains(&status) {
        Ok(v)
    } else {
        Err(crate::api_error(status, &v))
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
        let mut tools = vec![
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
                description: "How good the pushed feature map is, measured on 30 days of usage: a score out of 100, findings ranked by impact (pages no feature claims, overlapping routes, dead features, success events that never fire or fire on every use, data-feature values not declared, requires contradicted by usage, a feature whose users split into separate paths (split it), two features almost always used together (merge them), steps done in another order than declared, routes new in the latest release that no feature claims), each with evidence and a YAML fix, plus the relations inferred between features (lift, which comes first, leads to value). Nothing is applied: edit the file, then feature_map_push.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "map_overview",
                description: "Every feature with its area, status and numbers (uses, users, users who reached success; 30 days vs the 30 before), the roll-up by area and the adoption ladder. Start here, then feature_context for one feature.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "feature_context",
                description: "One feature in depth: its definition (area, requires, renamed_from), health and findings, users and success by role, top pages, the ladder rungs it is on, features that require it and its area peers, the features used with it (lift, which comes first, whether it leads to value) and its use per release, each counted with the map version in force when the release came out; `touched_by`: the releases whose changed files are in its code paths, with how many files.",
                input_schema: json!({"type":"object","properties":with(json!({"key":{"type":"string"}})),"required":["key"]}),
            },
            ToolSpec {
                name: "feature_relations",
                description: "Which features are used together, from every session of the last 30 days: per pair, the sessions using both, the lift (how many times more often than chance), which one usually comes first, and whether using the first raises the success of the second (leads to value, significant at p < 0.05). Pass `feature` to keep the pairs it is part of.",
                input_schema: json!({"type":"object","properties":with(json!({"feature":{"type":"string","description":"A feature key (optional)"}}))}),
            },
            ToolSpec {
                name: "suggest_map_changes",
                description: "Changes to make to openlens.features.yaml, ranked by impact, each with its evidence and a YAML patch. Nothing is applied: edit the file, then feature_map_push.",
                input_schema: json!({"type":"object","properties":with(json!({"limit":{"type":"integer","minimum":1,"maximum":50}}))}),
            },
            ToolSpec {
                name: "goals",
                description: "Goals on KPIs and objectives: value now, target, deadline, status (reached, on track, at risk, off track), guardrails and whether they are breached, what moved each goal (releases with their effect, flag/survey/map changes and notes since it was set), suggested targets per KPI, and `not_available`: the business KPIs this project cannot set a goal on yet, with why.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "set_goal",
                description: "Set a goal: a KPI key from `goals` suggestions (or success:<feature>), a target in KPI units (a share as 0-1). Business KPIs, offered only when their data exists: revenue (last 30 days, one currency), paid_conversion (needs plans: with a free and a paid plan), signup_rate (needs anonymous traffic, not cookieless), weekly_retention (week-1 retention of weekly cohorts), feature_adoption:<feature key of the map>; a refused one says why. Also a deadline (YYYY-MM-DD) or a number of releases. Optional guardrails: KPIs that must not get worse, with a limit (default 5% worse than now).",
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
                name: "releases",
                description: "The app's releases per environment over 90 days: state (rolling out, live at 95 % or more of the latest day's users, superseded by a newer release, rolled back when an older release took the majority back), share of the latest day, first and last seen, users, events, and for production the time in staging (from the same name or build fingerprint first seen in staging). With the deploy hook: each release's deploy time and last deploy status, the deploys recorded, and releases with no traffic whose deploy failed ('failed'), was rolled back, or is under way ('deploying'). Each release of production (or of env when given) also has its verdict against the release before it: healthy, regressed or inconclusive (null until 30 sessions and 8 users or 24 h), with its reasons; `release_report` gives the whole comparison. `notes`: release notes of the newest releases recorded with a commit range (merged pull requests read from GitHub hourly, grouped Added/Fixed/Changed, with counts, and `touched`: the features their changed files touched), or `unavailable` saying why there are none. Optional env for one environment.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "release_report",
                description: "One release compared with the release before it in its environment (production unless env): the verdict (healthy, regressed or inconclusive; null until 30 sessions and 8 users or 24 h) and its reasons, both releases' numbers, and every check with its outcome (worse, better, same, too_few, info): sessions with errors, bounce rate, sessions with a completed task, sessions per user, error groups first seen in the release (2 users or more), LCP/INP/CLS p75 ratings, success rate of each feature with a success event (the features the release's code change touched come first, before every other check, and one that cannot be compared says so; `current.touched` lists them), goal guardrails (breached by this release and not the one before) and goals (shown, not judged). `notes`: the release's notes from GitHub (state `ready`: its merged pull requests grouped Added/Fixed/Changed with number, title and URL, the range's commit, PR and file counts with `_more` for those not kept, the tag; any other state, with `reason`: no GitHub link, no commit range, not read yet, or the reads failed). `notes.touched`: the features whose map `paths` the release's changed files are in (state `ready`, with files per feature and `note` saying how many changed files match no feature), or `reason` why not (`no_paths`: the map in force for the release declares no code paths, see `openlens-cli rules`; `no_map`; `pending`); `notes.write_back`: what was written back to GitHub for a production release once its verdict is healthy or regressed (commit status `openlens/release`, a note in the GitHub release, one comment per pull request, an issue when regressed), with `state` (off, waiting, writing, written, denied when the token lacks a permission, failed) and `reason`. Read it after each deploy and act on a regressed verdict.",
                input_schema: json!({"type":"object","properties":with(json!({"release":{"type":"string"},"env":{"type":"string"}})),"required":["release"]}),
            },
            ToolSpec {
                name: "delivery",
                description: "The app's delivery metrics per environment over the last `days` (1 to 90, default 90): deploy frequency (succeeded deploys from the deploy hook, else releases reaching live), lead time from first commit to live (median; 'unknown' with why until a commit range is recorded), change failure rate (failed deploys, rollbacks seen by the hook or in traffic, regressed verdicts) and time to restore (median from a failure to the rollback or the next good change, with failures not restored yet). Optional env for one environment.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"days":{"type":"integer","minimum":1,"maximum":90}}))}),
            },
            ToolSpec {
                name: "deploy_hook",
                description: "Make or rotate the app's deploy hook URL (/hooks/deploy/SECRET), shown in this result only: the previous URL stops working at once. Paste it in the platform's deploy notifications (Coolify, Vercel, Netlify, Render, GitHub deployment_status, Argo CD, Flux), or post {status: started|succeeded|failed|rolled_back, release?, commit?, environment?, at?} to it from a deploy step; add ?env=NAME when the platform does not say the environment. Deploys then show in `releases`.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "record_release",
                description: "Record a release from the deploy step, after the deploy: its name (the one the SDK sends), the commit deployed and the commit its range starts after (default: the environment's previous deployed commit), and optionally when the range's first commit was made (first_commit_at, from `git log`), which gives the release's lead time in `delivery`. It shows in `releases` as a succeeded deploy. In CI prefer `openlens-cli release` or the openlens-release GitHub Action, which also upload the source maps.",
                input_schema: json!({"type":"object","properties":with(json!({"release":{"type":"string"},"commit":{"type":"string","description":"7 to 64 hex digits"},"from":{"type":"string","description":"7 to 64 hex digits"},"first_commit_at":{"type":"integer","description":"Author time (Unix seconds) of the oldest commit of the range, for the lead time"},"environment":{"type":"string","default":"production"}})),"required":["release"]}),
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
                name: "environments",
                description: "The app's environments: hosts per environment over 30 days (events, last seen, the rule that applies), the host rules, hosts no rule classifies yet (stored as 'unknown', counted in no production number), aliases, hidden environments and per-environment checks. Optional changes, applied first: host+env sets the rule for a host (exact or *.suffix; env is a name or 'ignore' to drop its events), remove removes it; alias+to stores an environment the app sends under another name ('ignore' drops it), unalias removes it; hide / show takes an environment out of the selector or back (its events stay readable by name); monitor+url (+level 'log' or 'notify', default log) checks a non-production environment at its own address every minute, unmonitor stops it. Rules and aliases apply to the next events, with no change to the app; relabel (a host or *.suffix, +days 1-90, default 30) then moves that host's past events to the environment the rules give now, as a dry run that only counts the moves unless dry_run is false. allow (a list of hosts or *.suffix, at most 50; [] turns it off) previews a host allow-list: the hosts outside it and their events over the last 24 h; with confirm true it is saved, and events of other hosts are then dropped at ingestion, but only while a listed host has sent events in the last 7 days (otherwise it is 'pending' and drops nothing); events without a page address always pass. The result's 'allow' shows the list, its status and the events it dropped per host.",
                input_schema: json!({"type":"object","properties":with(json!({"allow":{"type":"array","items":{"type":"string"},"maxItems":50},"confirm":{"type":"boolean","default":false},"relabel":{"type":"string"},"days":{"type":"integer","minimum":1,"maximum":90},"dry_run":{"type":"boolean","default":true},"host":{"type":"string"},"env":{"type":"string"},"remove":{"type":"string"},"alias":{"type":"string"},"to":{"type":"string"},"unalias":{"type":"string"},"hide":{"type":"string"},"show":{"type":"string"},"monitor":{"type":"string"},"url":{"type":"string"},"level":{"type":"string","enum":["log","notify"]},"unmonitor":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "verify",
                description: "Wiring checklist: events arrive, users identified with a role, release tagged, map pushed, events per feature, and `visitor linked to the person` (sends a test page view and identify, marked internal, in development; waits up to 45 s; says when the project has a single production host). Each failing check carries its fix. Also `release_layers`: which release layers are active (build fingerprint seen, deploy hook configured, releases recorded, verdicts present), each with how to turn it on.",
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
                description: "Headline KPIs in one compact payload: users (DAU/WAU/MAU, stickiness), sessions, vitals, top features and roles, latest two releases. With `by` (role|account|plan|page|feature), also users, sessions, events, tasks and errors per value (30 days). Roles are 'role at event time'. When the feature map declares `plans:`, also `plans`: upgrades and downgrades (last 30 days, the 30 before, per release) and plan gaps, plans being 'plan at event time'.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"},"by":{"type":"string","enum":["role","account","plan","page","feature"]}}))}),
            },
            ToolSpec {
                name: "user_timeline",
                description: "One user's story: role, account, plan, role and plan changes, recorded sessions (with dashboard links) and their latest 100 events (90 days). Use it to see what a churned or frustrated user did.",
                input_schema: json!({"type":"object","properties":with(json!({"distinct_id":{"type":"string"}})),"required":["distinct_id"]}),
            },
            ToolSpec {
                name: "board",
                description: "The Focus board numbers (KPIs by role/environment/release, features, roles, role gaps, releases, vitals). Period blocks read since the current release by default; `since` (a release) or `from`/`to` (YYYY-MM-DD, project days, at most 400 days, none in the future) pick another period, compared in `window` with the same length before. A period reading more than 30 days of events fails with its estimate (days, bytes, rough time) until called again with `confirm: \"1\"`.",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"},"since":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"},"confirm":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "board_job",
                description: "A long board period (a date range or past release that reads more than 30 days of events) computed in the background instead of in one call. `action: start` with the `board` tool's parameters returns the job's `id` (refused while you already have one queued or running; the error names it); `action: status` with `job` returns `state` (queued, running, done, failed, cancelled), `progress` (0 to 1, time against the estimate) and, once done, the `board`; `action: cancel` with `job` stops it. Jobs are kept an hour, for whoever started them.",
                input_schema: json!({"type":"object","properties":with(json!({"action":{"type":"string","enum":["start","status","cancel"]},"job":{"type":"string"},"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"},"since":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"}})),"required":["action"]}),
            },
            ToolSpec {
                name: "vitals",
                description: "Web Vitals explained (LCP, INP, CLS, FCP, TTFB), last 7 days: what each means for a visitor, p75, rating, good/poor thresholds, the worst pages at p75, the p75 per device class and the elements most often behind slow measurements (LCP, INP, CLS), the dominant cause with the part of the vital that weighs most (likely_cause), and what to do, most likely cause first.",
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
                description: "A one-time login link. Without email: an owner link for the PM (valid 24 hours). With email (+ role viewer|admin, optional projects): invites that person with that access (valid 7 days).",
                input_schema: json!({"type":"object","properties":{"email":{"type":"string"},"role":{"type":"string","enum":["viewer","admin"]},"projects":{"type":"array","items":{"type":"string"}}}}),
            },
        ];
        // Tools over the commands only a page ran before (#525).
        tools.extend(crate::mcp_commands::specs());
        // Planned maintenance (#563).
        tools.extend(crate::mcp_maintenance::specs());
        // Status announcements and incident updates (#562).
        tools.extend(crate::mcp_announcements::specs());
        // The public status page (#561).
        tools.extend(crate::mcp_status_page::specs());
        tools.extend(crate::agent_kit::specs(&proj));
        tools
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
            "feature_relations" => {
                let feature = s(args, "feature");
                if let Some(k) = feature
                    .as_deref()
                    .filter(|k| !openlens_featuremap::valid_key(k))
                {
                    return Err(format!("`{k}` is not a feature key"));
                }
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/feature-map/relations", project(args)),
                    None,
                )?;
                let mut v = ok_or(st, v)?;
                if let (Some(k), Some(list)) = (feature, v["relations"].as_array_mut()) {
                    list.retain(|r| r["first"] == k.as_str() || r["then"] == k.as_str());
                }
                Ok(v)
            }
            "goals" => {
                let (st, v) = c.call(
                    "GET",
                    &format!("/api/projects/{}/goals", project(args)),
                    None,
                )?;
                ok_or(st, v)
            }
            "releases" => {
                let mut path = format!("/api/projects/{}/releases", project(args));
                if let Some(e) = s(args, "env") {
                    let enc: String = e
                        .bytes()
                        .map(|b| {
                            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                                (b as char).to_string()
                            } else {
                                format!("%{b:02X}")
                            }
                        })
                        .collect();
                    path.push_str(&format!("?env={enc}"));
                }
                let (st, v) = c.call("GET", &path, None)?;
                ok_or(st, v)
            }
            "release_report" => {
                let enc = |v: &str| -> String {
                    v.bytes()
                        .map(|b| {
                            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                                (b as char).to_string()
                            } else {
                                format!("%{b:02X}")
                            }
                        })
                        .collect()
                };
                let Some(release) = s(args, "release") else {
                    return Err("release is required".into());
                };
                let mut path = format!(
                    "/api/projects/{}/releases/{}/report",
                    project(args),
                    enc(&release)
                );
                if let Some(e) = s(args, "env") {
                    path.push_str(&format!("?env={}", enc(&e)));
                }
                let (st, v) = c.call("GET", &path, None)?;
                ok_or(st, v)
            }
            "delivery" => {
                let mut q = Vec::new();
                if let Some(e) = s(args, "env") {
                    let enc: String = e
                        .bytes()
                        .map(|b| {
                            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                                (b as char).to_string()
                            } else {
                                format!("%{b:02X}")
                            }
                        })
                        .collect();
                    q.push(format!("env={enc}"));
                }
                if let Some(d) = args.get("days").and_then(Value::as_i64) {
                    q.push(format!("days={d}"));
                }
                let mut path = format!("/api/projects/{}/delivery", project(args));
                if !q.is_empty() {
                    path.push('?');
                    path.push_str(&q.join("&"));
                }
                let (st, v) = c.call("GET", &path, None)?;
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
            "record_release" => {
                let mut body = args.clone();
                if let Some(o) = body.as_object_mut() {
                    o.remove("project");
                }
                let (st, v) = c.call(
                    "POST",
                    &format!("/api/projects/{}/releases", project(args)),
                    Some(Body::Json(body)),
                )?;
                ok_or(st, v)
            }
            "deploy_hook" => {
                let (st, v) = c.call(
                    "POST",
                    &format!("/api/projects/{}/deploy-hook", project(args)),
                    None,
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
            "environments" => {
                let p = project(args);
                let change = environment_change(args)?;
                if let Some(body) = change {
                    let (st, v) = c.call(
                        "PATCH",
                        &format!("/api/projects/{p}"),
                        Some(Body::Json(body)),
                    )?;
                    ok_or(st, v)?;
                }
                // Re-labelling: a dry run unless the agent says otherwise.
                let relabelled = match s(args, "relabel") {
                    Some(host) => {
                        let mut body = json!({
                            "host": host,
                            "dry_run": args.get("dry_run").and_then(Value::as_bool).unwrap_or(true),
                        });
                        if let Some(d) = args.get("days").and_then(Value::as_i64) {
                            body["days"] = json!(d);
                        }
                        let (st, v) = c.call(
                            "POST",
                            &format!("/api/projects/{p}/environments"),
                            Some(Body::Json(body)),
                        )?;
                        Some(ok_or(st, v)?)
                    }
                    None => None,
                };
                // The allow-list: a preview unless the agent confirms.
                let allowed = match args.get("allow").and_then(Value::as_array) {
                    Some(hosts) => {
                        let body = json!({
                            "hosts": hosts,
                            "confirm": args.get("confirm").and_then(Value::as_bool).unwrap_or(false),
                        });
                        let (st, v) = c.call(
                            "POST",
                            &format!("/api/projects/{p}/environments/allow"),
                            Some(Body::Json(body)),
                        )?;
                        Some(ok_or(st, v)?)
                    }
                    None => None,
                };
                let (st, v) = c.call("GET", &format!("/api/projects/{p}/environments"), None)?;
                let mut v = ok_or(st, v)?;
                if let Some(r) = relabelled {
                    v["relabel"] = r;
                }
                if let Some(a) = allowed {
                    v["allow_preview"] = a["preview"].clone();
                    v["allow_saved"] = a["saved"].clone();
                }
                Ok(v)
            }
            "verify" => crate::verify_report(c, &project(args)),
            "suggested_features" | "set_feature" | "stale_flags" => {
                crate::agent_kit::tool(c, name, args)
            }
            "insights" | "kpi_report" | "board" => {
                let mut q = Vec::new();
                let keys: &[&str] = if name == "board" {
                    &["env", "role", "release", "since", "from", "to", "confirm"]
                } else {
                    &["env", "role", "release"]
                };
                for k in keys {
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
                        let mut report = json!({
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
                        });
                        // Plans (#712, D-113): only when the map declares them.
                        if !v["plans"].is_null() {
                            report["plans"] = v["plans"].clone();
                            report["plan_rule"] = json!("plan at event time");
                        }
                        report
                    }
                    _ => v,
                })
            }
            "board_job" => {
                let base = format!("/api/projects/{}/board/jobs", project(args));
                let job = || -> Result<String, String> {
                    let id: String = s(args, "job")
                        .ok_or("job is required")?
                        .chars()
                        .filter(char::is_ascii_alphanumeric)
                        .collect();
                    Ok(format!("{base}/{id}"))
                };
                let (st, v) = match s(args, "action").as_deref() {
                    Some("start") => {
                        let q: Vec<String> = ["env", "role", "release", "since", "from", "to"]
                            .iter()
                            .filter_map(|k| {
                                s(args, k)
                                    .map(|v| format!("{k}={}", v.replace(['&', '#', '?', ' '], "")))
                            })
                            .collect();
                        c.call("POST", &format!("{base}?{}", q.join("&")), None)?
                    }
                    Some("status") => c.call("GET", &job()?, None)?,
                    Some("cancel") => c.call("DELETE", &job()?, None)?,
                    _ => return Err("action is start, status or cancel".into()),
                };
                match v.get("job").and_then(Value::as_str) {
                    Some(running) if st == 429 => Err(format!(
                        "{} (your job: {running})",
                        crate::api_error(st, &v)
                    )),
                    _ => ok_or(st, v),
                }
            }
            "vitals" => {
                let mut q = Vec::new();
                let keys: &[&str] = if name == "board" {
                    &["env", "role", "release", "since", "from", "to"]
                } else {
                    &["env", "role", "release"]
                };
                for k in keys {
                    if let Some(v) = s(args, k) {
                        q.push(format!("{k}={}", v.replace(['&', '#', '?', ' '], "")));
                    }
                }
                let path = format!("/api/projects/{}/vitals?{}", project(args), q.join("&"));
                let (st, v) = c.call("GET", &path, None)?;
                ok_or(st, v)
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
            // Tools over the commands only a page ran before (#525).
            _ => crate::mcp_commands::call(c, name, args)
                .or_else(|| crate::mcp_maintenance::call(c, name, args))
                .or_else(|| crate::mcp_announcements::call(c, name, args))
                .or_else(|| crate::mcp_status_page::call(c, name, args))
                .unwrap_or_else(|| Err(format!("unknown tool {name}"))),
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
