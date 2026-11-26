//! `openlens-cli`: the agent toolkit (docs/agents/wire.md).
//!
//! Every command prints one JSON document on stdout, so an agent can parse
//! it, and exits non-zero when something needs fixing. The server and the
//! admin token come from `--server`/`--token` or `OPENLENS_SERVER` /
//! `OPENLENS_TOKEN` (no config file, D-014).

mod convert;
mod lint;
mod mcp;
#[cfg(test)]
mod mcp_parity;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

mod commands;
mod release;
mod verify;
use commands::*;
pub(crate) use verify::*;

#[derive(Parser)]
#[command(
    name = "openlens-cli",
    version,
    about = "OpenLens agent toolkit (prints JSON)"
)]
struct Cli {
    /// OpenLens server URL.
    #[arg(long, env = "OPENLENS_SERVER", global = true)]
    server: Option<String>,
    /// Admin token.
    #[arg(long, env = "OPENLENS_TOKEN", global = true, hide_env_values = true)]
    token: Option<String>,
    /// Read times and days in this IANA zone (default: each project's).
    #[arg(long, env = "OPENLENS_TZ", global = true)]
    tz: Option<String>,
    #[command(subcommand)]
    command: Command,
}

/// Percent-encode a query value.
fn query_value(v: &str) -> String {
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

/// Plain HTTP is accepted only where the token never crosses the internet:
/// this machine, and addresses that only resolve inside a private network
/// or a cluster (a Kubernetes Service name, `*.svc`, `*.cluster.local`,
/// `*.internal`, private IPv4 ranges).
fn plain_http_ok(server: &str) -> bool {
    let Some(rest) = server.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
    .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return ip.is_loopback() || ip.is_private();
    }
    host == "localhost"
        || !host.contains('.')
        || [".svc", ".cluster.local", ".internal", ".localhost"]
            .iter()
            .any(|s| host.ends_with(s))
}

pub(crate) struct Client {
    server: String,
    token: String,
    agent: ureq::Agent,
    /// Zone added as `tz=` to project reads (`--tz`).
    tz: Option<String>,
}

impl Client {
    fn new(server: Option<String>, token: Option<String>) -> Result<Self, String> {
        let server = server
            .ok_or("missing --server (or OPENLENS_SERVER)")?
            .trim_end_matches('/')
            .to_owned();
        if !(server.starts_with("https://") || plain_http_ok(&server)) {
            return Err(
                "the server must use https (plain http is only allowed for localhost and cluster-internal addresses)".into(),
            );
        }
        let token = token.ok_or("missing --token (or OPENLENS_TOKEN)")?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Ok(Self {
            server,
            token,
            agent,
            tz: None,
        })
    }

    pub(crate) fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Body>,
    ) -> Result<(u16, Value), String> {
        let mut url = format!("{}{path}", self.server);
        if let Some(tz) = &self.tz
            && method == "GET"
            && path.starts_with("/api/projects/")
        {
            url.push(if path.contains('?') { '&' } else { '?' });
            url.push_str("tz=");
            url.push_str(&tz.replace('+', "%2B"));
        }
        let auth = format!("Bearer {}", self.token);
        let resp = match (method, body) {
            ("GET", _) => self.agent.get(&url).header("authorization", &auth).call(),
            ("DELETE", _) => self
                .agent
                .delete(&url)
                .header("authorization", &auth)
                .call(),
            ("PATCH", Some(Body::Json(v))) => self
                .agent
                .patch(&url)
                .header("authorization", &auth)
                .send_json(&v),
            ("PUT", Some(Body::Json(v))) => self
                .agent
                .put(&url)
                .header("authorization", &auth)
                .send_json(&v),
            (_, Some(Body::Json(v))) => self
                .agent
                .post(&url)
                .header("authorization", &auth)
                .send_json(&v),
            ("PUT", Some(Body::Text(t))) => self
                .agent
                .put(&url)
                .header("authorization", &auth)
                .send(t.as_bytes()),
            (_, Some(Body::Raw(t))) => self
                .agent
                .post(&url)
                .header("authorization", &auth)
                .send(t.as_bytes()),
            _ => self
                .agent
                .post(&url)
                .header("authorization", &auth)
                .send_empty(),
        }
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .into_body()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| e.to_string())?;
        let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Ok((status, value))
    }

    pub(crate) fn sql(&self, project: &str, query: &str) -> Result<Vec<Value>, String> {
        let (status, v) = self.call(
            "POST",
            &format!("/api/projects/{project}/sql"),
            Some(Body::Json(json!({ "sql": query }))),
        )?;
        if status != 200 {
            return Err(format!("sql failed: {}", api_error(status, &v)));
        }
        Ok(v["rows"].as_array().cloned().unwrap_or_default())
    }
}

/// An API error as one line, from its stable `code` (#312): texts may
/// change between releases, codes do not.
pub(crate) fn api_error(status: u16, v: &Value) -> String {
    let message = v["message"]
        .as_str()
        .or_else(|| v["error"].as_str())
        .map_or_else(|| v.to_string(), str::to_owned);
    match v["code"].as_str() {
        Some("unauthorized") => format!("the token was refused (HTTP {status}): {message}"),
        Some("forbidden") => format!("this token may not do that (HTTP {status}): {message}"),
        Some("not_found") => format!("not found (HTTP {status}): {message}"),
        Some("busy" | "rate_limited" | "unavailable") => {
            format!("the server is busy, try again shortly (HTTP {status}): {message}")
        }
        Some(code) => format!("{message} ({code}, HTTP {status})"),
        None => format!("HTTP {status}: {message}"),
    }
}

pub(crate) enum Body {
    Json(Value),
    Text(String),
    Raw(String),
}

fn out(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn run(cli: Cli) -> Result<bool, String> {
    let client = || {
        Client::new(cli.server.clone(), cli.token.clone()).map(|mut c| {
            c.tz = cli.tz.clone().filter(|t| !t.is_empty());
            c
        })
    };
    match &cli.command {
        Command::DeleteProject { id, confirm } => {
            let (status, v) = client()?.call(
                "DELETE",
                &format!("/api/projects/{id}?confirm={confirm}"),
                None,
            )?;
            out(&v);
            Ok(status == 200)
        }
        Command::Init { name, timezone } => {
            let mut body = json!({ "name": name });
            if let Some(tz) = timezone {
                body["timezone"] = json!(tz);
            }
            let (status, v) = client()?.call("POST", "/api/projects", Some(Body::Json(body)))?;
            out(&v);
            Ok(status == 201)
        }
        Command::Timezone { project, timezone } => {
            let (status, v) = client()?.call(
                "PATCH",
                &format!("/api/projects/{project}"),
                Some(Body::Json(json!({ "timezone": timezone }))),
            )?;
            out(&v);
            Ok(status == 200)
        }
        Command::Features {
            action:
                FeaturesCmd::Lint {
                    file,
                    src,
                    live,
                    project,
                    min_score,
                },
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let mut report = lint::lint(&text, src);
            let mut ok = report["valid"] == true && report["errors"].as_u64() == Some(0);
            if *live {
                let (status, h) = client()?.call(
                    "GET",
                    &format!("/api/projects/{project}/feature-map/health"),
                    None,
                )?;
                if status != 200 {
                    ok = false;
                } else if let Some(min) = min_score {
                    ok &= h["score"].as_u64().unwrap_or(0) >= u64::from(*min);
                }
                report["live"] = h;
            }
            out(&report);
            Ok(ok)
        }
        Command::Features {
            action:
                FeaturesCmd::Push {
                    file,
                    project,
                    message,
                },
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let q = message
                .as_deref()
                .map(|m| format!("?message={}", query_value(m)))
                .unwrap_or_default();
            let (status, v) = client()?.call(
                "PUT",
                &format!("/api/projects/{project}/feature-map{q}"),
                Some(Body::Text(text)),
            )?;
            out(&v);
            Ok(status == 200)
        }
        Command::Features {
            action: FeaturesCmd::Context { key, project },
        } => {
            let (status, v) = client()?.call(
                "GET",
                &format!(
                    "/api/projects/{project}/feature-map/features/{}",
                    query_value(key)
                ),
                None,
            )?;
            out(&v);
            Ok(status == 200)
        }
        Command::Verify { project } => verify(&client()?, project),
        Command::Sourcemaps {
            project,
            release,
            dir,
        } => release::sourcemaps(&client()?, project, release, dir),
        Command::Import {
            project,
            file,
            format,
            map,
        } => {
            let raw = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let (text, conversion_errors) = match format {
                Some(f) => {
                    let (lines, errors) = convert::convert(&raw, f, &convert::mapping(map)?)?;
                    (lines.join("\n"), errors)
                }
                None => (raw, Vec::new()),
            };
            let c = client()?;
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let (mut events, mut persons, mut skipped, mut rejected) =
                (0i64, 0i64, 0usize, Vec::new());
            for chunk in lines.chunks(5_000) {
                let (status, v) = c.call(
                    "POST",
                    &format!("/api/projects/{project}/import"),
                    Some(Body::Raw(chunk.join("\n"))),
                )?;
                match status {
                    200 => {
                        events += v["accepted_events"].as_i64().unwrap_or(0);
                        persons += v["persons"].as_i64().unwrap_or(0);
                        rejected.extend(
                            v["rejected"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .take(20),
                        );
                    }
                    409 => skipped += 1,
                    _ => return Err(format!("import failed ({status}): {v}")),
                }
            }
            out(
                &json!({ "events": events, "persons": persons, "chunks_already_imported": skipped,
                "rejected_sample": rejected,
                "unreadable_rows": conversion_errors.len(),
                "unreadable_sample": conversion_errors.iter().take(20).collect::<Vec<_>>() }),
            );
            Ok(true)
        }
        Command::Export {
            project,
            from,
            to,
            out: path,
        } => {
            use std::io::Write;
            let parse = |d: &str| {
                let p: Vec<u32> = d.split('-').filter_map(|x| x.parse().ok()).collect();
                match p.as_slice() {
                    [y, m, dd] if (1..=12).contains(m) && (1..=31).contains(dd) => {
                        Ok((*y as i32, *m, *dd))
                    }
                    _ => Err(format!("{d} is not YYYY-MM-DD")),
                }
            };
            let (from, to) = (parse(from)?, parse(to)?);
            let days = days_between(from, to)?;
            let c = client()?;
            let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            let (mut rows, mut truncated) = (0usize, Vec::new());
            for day in days {
                let (status, v) = c.call(
                    "GET",
                    &format!("/api/projects/{project}/export?date={day}"),
                    None,
                )?;
                if status != 200 {
                    return Err(format!("export of {day} failed: {}", api_error(status, &v)));
                }
                let text = match &v {
                    Value::String(s) => s.clone(),
                    other => format!("{other}\n"),
                };
                if text.lines().count() >= 500_000 {
                    truncated.push(day.clone());
                }
                rows += text.lines().filter(|l| !l.is_empty()).count();
                file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            }
            out(
                &json!({ "rows": rows, "file": path.display().to_string(), "days_truncated": truncated }),
            );
            Ok(true)
        }
        Command::Annotate { project, note, day } => {
            let path = format!("/api/projects/{project}/timeline");
            let (status, v) = match note {
                Some(n) => client()?.call(
                    "POST",
                    &path,
                    Some(Body::Json(json!({ "body": n, "day": day }))),
                )?,
                None => client()?.call("GET", &path, None)?,
            };
            out(&v);
            Ok(status == 200 || status == 201)
        }
        Command::Env { action } => {
            let c = client()?;
            let (status, v) = match action {
                EnvCmd::List { project } => c.call(
                    "GET",
                    &format!("/api/projects/{project}/environments"),
                    None,
                )?,
                EnvCmd::Rule { host, env, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(
                        json!({ "environment_rule": { "host": host, "env": env } }),
                    )),
                )?,
                EnvCmd::Relabel {
                    host,
                    days,
                    dry_run,
                    project,
                } => c.call(
                    "POST",
                    &format!("/api/projects/{project}/environments"),
                    Some(Body::Json(
                        json!({ "host": host, "days": days, "dry_run": dry_run }),
                    )),
                )?,
                EnvCmd::Allow {
                    hosts,
                    confirm,
                    project,
                } => c.call(
                    "POST",
                    &format!("/api/projects/{project}/environments/allow"),
                    Some(Body::Json(json!({ "hosts": hosts, "confirm": confirm }))),
                )?,
                EnvCmd::Unrule { host, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(json!({ "remove_environment_rule": host }))),
                )?,
                EnvCmd::Alias { from, to, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(
                        json!({ "environment_alias": { "from": from, "to": to } }),
                    )),
                )?,
                EnvCmd::Unalias { from, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(json!({ "remove_environment_alias": from }))),
                )?,
                EnvCmd::Hide { env, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(json!({ "hide_environment": env }))),
                )?,
                EnvCmd::Show { env, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(json!({ "show_environment": env }))),
                )?,
                EnvCmd::Monitor {
                    env,
                    url,
                    level,
                    project,
                } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(
                        json!({ "environment_monitor": { "env": env, "url": url, "level": level } }),
                    )),
                )?,
                EnvCmd::Unmonitor { env, project } => c.call(
                    "PATCH",
                    &format!("/api/projects/{project}"),
                    Some(Body::Json(json!({ "remove_environment_monitor": env }))),
                )?,
            };
            out(&v);
            Ok(status == 200)
        }
        Command::Goals { project } => {
            let (status, v) =
                client()?.call("GET", &format!("/api/projects/{project}/goals"), None)?;
            out(&v);
            Ok(status == 200)
        }
        Command::Status { project } => {
            let (status, v) =
                client()?.call("GET", &format!("/api/projects/{project}/status"), None)?;
            out(&v);
            Ok(status == 200 && v["state"] != "down")
        }
        Command::Flows {
            project,
            from,
            to_success,
        } => {
            let mut q = Vec::new();
            if let Some(f) = from {
                q.push(format!("from={}", f.replace(['&', '#', ' '], "")));
            }
            if *to_success {
                q.push("to=success".to_owned());
            }
            let (status, v) = client()?.call(
                "GET",
                &format!("/api/projects/{project}/flows?{}", q.join("&")),
                None,
            )?;
            out(&v);
            Ok(status == 200)
        }
        Command::Comments {
            project,
            session,
            at,
            add,
        } => {
            let c = client()?;
            let path = format!("/api/projects/{project}/replays/{session}/comments");
            let (status, v) = match add {
                Some(body) => c.call(
                    "POST",
                    &path,
                    Some(Body::Json(json!({ "at": at, "body": body }))),
                )?,
                None => c.call("GET", &path, None)?,
            };
            out(&v);
            Ok(status == 200 || status == 201)
        }
        Command::Users { action } => {
            let c = client()?;
            let (status, v) = match action {
                UsersCmd::List => c.call("GET", "/api/users", None)?,
                UsersCmd::Update {
                    id,
                    name,
                    email,
                    role,
                    projects,
                    timezone,
                } => {
                    let mut b = serde_json::Map::new();
                    for (k, v) in [
                        ("name", name),
                        ("email", email),
                        ("role", role),
                        ("timezone", timezone),
                    ] {
                        if let Some(v) = v {
                            b.insert(k.into(), json!(v));
                        }
                    }
                    match projects.as_deref() {
                        Some("all") => {
                            b.insert("all_projects".into(), json!(true));
                        }
                        Some(p) => {
                            b.insert(
                                "projects".into(),
                                json!(
                                    p.split(',')
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                        .collect::<Vec<_>>()
                                ),
                            );
                        }
                        None => {}
                    }
                    c.call(
                        "PATCH",
                        &format!("/api/users/{id}"),
                        Some(Body::Json(Value::Object(b))),
                    )?
                }
                UsersCmd::Disable { id } | UsersCmd::Enable { id } => {
                    let off = matches!(action, UsersCmd::Disable { .. });
                    c.call(
                        "PATCH",
                        &format!("/api/users/{id}"),
                        Some(Body::Json(json!({ "disabled": off }))),
                    )?
                }
                UsersCmd::Delete { id } => c.call("DELETE", &format!("/api/users/{id}"), None)?,
            };
            out(&v);
            Ok(status == 200)
        }
        Command::Tokens { action } => {
            let c = client()?;
            let (status, v) = match action {
                TokensCmd::List => c.call("GET", "/api/tokens", None)?,
                TokensCmd::Create {
                    scope,
                    projects,
                    label,
                } => c.call(
                    "POST",
                    "/api/tokens",
                    Some(Body::Json(
                        json!({ "scope": scope, "projects": projects, "label": label }),
                    )),
                )?,
                TokensCmd::Revoke { id } => c.call("DELETE", &format!("/api/tokens/{id}"), None)?,
            };
            out(&v);
            Ok(status == 200)
        }
        Command::Flags { action } => {
            let c = client()?;
            let (status, v) = match action {
                FlagsCmd::List { project } => {
                    c.call("GET", &format!("/api/projects/{project}/flags"), None)?
                }
                FlagsCmd::Set {
                    key,
                    project,
                    rollout,
                    off,
                    roles,
                    accounts,
                    description,
                    rules,
                } => {
                    let rules = rules
                        .iter()
                        .map(|r| openlens_flags::Rule::parse(r))
                        .collect::<Result<Vec<_>, String>>()?;
                    c.call(
                        "PUT",
                        &format!("/api/projects/{project}/flags/{key}"),
                        Some(Body::Json(json!({
                            "key": key, "enabled": !off, "rollout": rollout, "roles": roles,
                            "accounts": accounts, "description": description, "rules": rules,
                        }))),
                    )?
                }
                FlagsCmd::Delete { key, project } => c.call(
                    "DELETE",
                    &format!("/api/projects/{project}/flags/{key}"),
                    None,
                )?,
            };
            out(&v);
            Ok(status == 200)
        }
        Command::Surveys { action } => {
            let c = client()?;
            let (status, v) = match action {
                SurveysCmd::List { project } => {
                    c.call("GET", &format!("/api/projects/{project}/surveys"), None)?
                }
                SurveysCmd::Set {
                    key,
                    project,
                    question,
                    kind,
                    choices,
                    trigger,
                    sample,
                    roles,
                    off,
                } => c.call(
                    "PUT",
                    &format!("/api/projects/{project}/surveys/{key}"),
                    Some(Body::Json(json!({
                        "key": key, "question": question, "kind": kind, "choices": choices,
                        "trigger": trigger, "sample": sample, "roles": roles, "enabled": !off,
                    }))),
                )?,
                SurveysCmd::Delete { key, project } => c.call(
                    "DELETE",
                    &format!("/api/projects/{project}/surveys/{key}"),
                    None,
                )?,
            };
            out(&v);
            Ok(status == 200)
        }
        Command::Rules => {
            println!("{}", RULES_SNIPPET);
            Ok(true)
        }
        Command::LoginLink {
            email,
            role,
            projects,
        } => {
            let body = email.as_ref().map(|e| {
                Body::Json(json!({
                    "email": e,
                    "role": role,
                    "projects": projects.as_ref().map(|p| p.split(',').map(str::trim).collect::<Vec<_>>()),
                }))
            });
            let (status, v) = client()?.call("POST", "/api/login-links", body)?;
            out(&v);
            Ok(status == 200)
        }
        Command::Sql { project, query } => {
            let rows = client()?.sql(project, query)?;
            out(&json!({ "rows": rows }));
            Ok(true)
        }
        Command::Guide => {
            let c = client().or_else(|_| Client::new(cli.server.clone(), Some(String::new())))?;
            let (status, v) = c.call("GET", "/agents.md", None)?;
            println!("{}", v.as_str().unwrap_or_default());
            Ok(status == 200)
        }
        Command::Mcp {
            automation_read_root,
            automation_write_root,
        } => {
            let tools =
                mcp::OpenLensTools::new(client()?, automation_read_root, automation_write_root)?;
            openlens_mcp::serve(&tools, "openlens", env!("CARGO_PKG_VERSION"))
                .map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            out(&json!({ "error": e }));
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod http_tests {
    use super::{api_error, plain_http_ok};

    /// `/api/projects/{project}/flows?{}` as `/api/projects/{}/flows`.
    fn template(path: &str) -> String {
        path.split('/')
            .map(|seg| {
                if seg.starts_with('{') {
                    "{}".to_owned()
                } else {
                    seg.split(['?', '{']).next().unwrap_or("").to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Every API path the CLI and its MCP tools call is in the server's
    /// OpenAPI document (#312), so a renamed route fails here, not in use.
    #[test]
    fn every_path_the_cli_calls_is_in_the_contract() {
        let doc = openlens_server::openapi_document();
        let known: std::collections::BTreeSet<String> = doc["paths"]
            .as_object()
            .unwrap()
            .keys()
            .map(|p| template(&format!("/api{p}")))
            .collect();
        let mut checked = 0;
        for src in [include_str!("main.rs"), include_str!("mcp.rs")] {
            for lit in src
                .split('"')
                .filter(|l| l.starts_with("/api/") && !l.ends_with('/'))
            {
                assert!(
                    known.contains(&template(lit)),
                    "{lit} is not in /api/openapi.json"
                );
                checked += 1;
            }
        }
        assert!(checked > 40, "found only {checked} paths");
    }

    #[test]
    fn errors_are_read_from_their_code_not_their_text() {
        let v =
            serde_json::json!({"code": "not_found", "message": "unknown project", "error": "x"});
        assert_eq!(api_error(404, &v), "not found (HTTP 404): unknown project");
        let old = serde_json::json!({"error": "boom"});
        assert_eq!(api_error(400, &old), "HTTP 400: boom");
        let busy = serde_json::json!({"code": "busy", "message": "m"});
        assert!(api_error(503, &busy).starts_with("the server is busy"));
    }

    #[test]
    fn plain_http_only_stays_inside() {
        for ok in [
            "http://localhost:8686",
            "http://127.0.0.1:8686",
            "http://lens-openlens:8686",
            "http://lens-openlens.default.svc:8686",
            "http://lens-openlens.default.svc.cluster.local:8686/",
            "http://10.0.3.7:8686",
            "http://192.168.1.20",
        ] {
            assert!(plain_http_ok(ok), "{ok}");
        }
        for no in [
            "http://lens.example.com",
            "http://8.8.8.8:8686",
            "http://evil.com@lens-openlens:8686.example.com",
            "http://svc.example.com",
            "https://lens.example.com",
            "http://",
        ] {
            assert!(!plain_http_ok(no), "{no}");
        }
    }
}
