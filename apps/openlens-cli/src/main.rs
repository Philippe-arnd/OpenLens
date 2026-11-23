//! `openlens-cli`: the agent toolkit (docs/agents/wire.md).
//!
//! Every command prints one JSON document on stdout, so an agent can parse
//! it, and exits non-zero when something needs fixing. The server and the
//! admin token come from `--server`/`--token` or `OPENLENS_SERVER` /
//! `OPENLENS_TOKEN` (no config file, D-014).

mod lint;
mod mcp;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

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

#[derive(Subcommand)]
enum UsersCmd {
    /// Every user, with role, projects, time zone and last sign-in.
    List,
    /// Change a user's name, email, role, projects or time zone.
    Update {
        /// User id (from `users list`).
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        email: Option<String>,
        /// viewer or admin.
        #[arg(long)]
        role: Option<String>,
        /// Comma-separated project ids, or `all`.
        #[arg(long)]
        projects: Option<String>,
        /// IANA zone they read times in (empty: the project's).
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Refuse sign-in (their sessions end at once).
    Disable { id: String },
    /// Allow sign-in again.
    Enable { id: String },
    /// Remove a user (never the last admin).
    Delete { id: String },
}

#[derive(Subcommand)]
enum Command {
    /// Create a project; prints its capture key and the snippet to add.
    Init {
        /// Project name (usually the app's name).
        #[arg(long)]
        name: String,
        /// The team's IANA time zone (`Europe/Paris`): days and weeks of
        /// every KPI are counted in it. UTC when omitted.
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Set a project's time zone (IANA name, e.g. `Europe/Paris`).
    Timezone {
        /// Project id.
        project: String,
        /// IANA time zone name.
        timezone: String,
    },
    /// Delete a project and all its data (events, replays, people, flags,
    /// surveys, feature map). Irreversible: repeat the id with --confirm.
    DeleteProject {
        /// Project id.
        id: String,
        /// The same id again.
        #[arg(long)]
        confirm: String,
    },
    /// Feature map commands.
    Features {
        #[command(subcommand)]
        action: FeaturesCmd,
    },
    /// Check the wiring: events arrive, roles resolve, features fire.
    Verify {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Backfill history from the app's own data (JSONL: persons and past events).
    Import {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// JSONL file. Person lines: {"type":"person","id","role","account","plan","created_at","last_seen_at"}.
        /// Event lines: {"event","distinct_id","timestamp","properties"}.
        #[arg(long)]
        file: PathBuf,
    },
    /// Export raw events as JSONL, day by day, for deep analysis.
    Export {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// First day, YYYY-MM-DD.
        #[arg(long)]
        from: String,
        /// Last day, YYYY-MM-DD (inclusive).
        #[arg(long)]
        to: String,
        /// Output file (JSONL).
        #[arg(long)]
        out: PathBuf,
    },
    /// Micro-surveys: list (with results), set (create or update) and delete.
    Surveys {
        #[command(subcommand)]
        action: SurveysCmd,
    },
    /// Feature flags: list, set (create or update) and delete.
    Flags {
        #[command(subcommand)]
        action: FlagsCmd,
    },
    /// How sessions move between features (Sankey data, last 30 days).
    Flows {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Start at this feature.
        #[arg(long)]
        from: Option<String>,
        /// Only sessions that reached a success event.
        #[arg(long)]
        to_success: bool,
    },
    /// Comments pinned to moments of a replay: list them, or add one with
    /// --at and --add.
    Comments {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Session id.
        session: String,
        /// Seconds into the recording (with --add).
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        /// Add this comment.
        #[arg(long)]
        add: Option<String>,
    },
    /// The team: list, update, disable, enable or remove users (invite
    /// with `login-link --email`).
    Users {
        #[command(subcommand)]
        action: UsersCmd,
    },
    /// Upload the source maps of a build, so error stacks show original
    /// files and lines. Run it in the deploy step, with the same release
    /// the SDK sends.
    Sourcemaps {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Release of this build (as in `init({ release })` / data-release).
        #[arg(long)]
        release: String,
        /// Build output directory; every `*.map` file under it is uploaded.
        dir: PathBuf,
    },
    /// Print the rule to add to the app's AGENTS.md / CLAUDE.md so its agents
    /// keep the feature map current.
    Rules,
    /// Create a one-time login link for the PM (valid 7 days).
    LoginLink {
        /// Invite this person (else: an owner link, full access).
        #[arg(long)]
        email: Option<String>,
        /// Their role: viewer (look) or admin (change flags, invite).
        #[arg(long, default_value = "viewer")]
        role: String,
        /// Comma-separated project ids they may see (default: all).
        #[arg(long)]
        projects: Option<String>,
    },
    /// Run read-only SQL on a project's `events` table.
    Sql {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// The query.
        query: String,
    },
    /// Print the agent wiring guide served by the server.
    Guide,
    /// Run the MCP server for agents. Both roots are required: the server
    /// refuses any file access outside them.
    Mcp {
        /// Directory agents may read from.
        #[arg(long, required = true)]
        automation_read_root: PathBuf,
        /// Directory agents may write to.
        #[arg(long, required = true)]
        automation_write_root: PathBuf,
    },
}

#[derive(Subcommand)]
enum SurveysCmd {
    /// List the project's surveys and their results.
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Create or update a survey.
    Set {
        /// Survey key.
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// The question (at most 200 characters).
        #[arg(long)]
        question: String,
        /// rating (1-5), yes_no, choice or text.
        #[arg(long)]
        kind: String,
        /// Options for kind=choice (comma-separated, 2-6).
        #[arg(long, value_delimiter = ',')]
        choices: Vec<String>,
        /// Event after which it is shown, e.g. entry_saved.
        #[arg(long)]
        trigger: String,
        /// Share of eligible users asked, 0-100.
        #[arg(long, default_value_t = 10)]
        sample: u8,
        /// Only these canonical roles (comma-separated).
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
        /// Create or leave it switched off.
        #[arg(long)]
        off: bool,
    },
    /// Delete a survey.
    Delete {
        /// Survey key.
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

#[derive(Subcommand)]
enum FlagsCmd {
    /// List the project's flags.
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Create or update a flag.
    Set {
        /// Flag key, as used in code: openlens.isEnabled("<key>").
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Share of eligible users, 0-100.
        #[arg(long, default_value_t = 0)]
        rollout: u8,
        /// Create or leave it switched off.
        #[arg(long)]
        off: bool,
        /// Only these canonical roles (comma-separated).
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
        /// Always on for these accounts (comma-separated).
        #[arg(long, value_delimiter = ',')]
        accounts: Vec<String>,
        /// What the flag is for.
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Delete a flag.
    Delete {
        /// Flag key.
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

#[derive(Subcommand)]
enum FeaturesCmd {
    /// Validate the map offline and check it against the app's code.
    Lint {
        /// Path to the map.
        #[arg(long, default_value = "openlens.features.yaml")]
        file: PathBuf,
        /// App source root to scan for data-feature attributes and routes.
        #[arg(long, default_value = ".")]
        src: PathBuf,
    },
    /// Validate and upload the map (new version on the server).
    Push {
        /// Path to the map.
        #[arg(long, default_value = "openlens.features.yaml")]
        file: PathBuf,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
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
            return Err(format!("sql failed ({status}): {v}"));
        }
        Ok(v["rows"].as_array().cloned().unwrap_or_default())
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
            action: FeaturesCmd::Lint { file, src },
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let report = lint::lint(&text, src);
            let ok = report["valid"] == true && report["errors"].as_u64() == Some(0);
            out(&report);
            Ok(ok)
        }
        Command::Features {
            action: FeaturesCmd::Push { file, project },
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let (status, v) = client()?.call(
                "PUT",
                &format!("/api/projects/{project}/feature-map"),
                Some(Body::Text(text)),
            )?;
            out(&v);
            Ok(status == 200)
        }
        Command::Verify { project } => verify(&client()?, project),
        Command::Sourcemaps {
            project,
            release,
            dir,
        } => {
            let c = client()?;
            let mut maps = Vec::new();
            let mut stack = vec![dir.clone()];
            while let Some(d) = stack.pop() {
                let entries = std::fs::read_dir(&d)
                    .map_err(|e| format!("cannot read {}: {e}", d.display()))?;
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x == "map") {
                        maps.push(p);
                    }
                }
            }
            let rel: String = release
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || "-_.@+".contains(ch) {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect();
            let (mut ok, mut failed) = (Vec::new(), Vec::new());
            for p in &maps {
                let Some(bundle) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let text = std::fs::read_to_string(p)
                    .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
                let (status, v) = c.call(
                    "PUT",
                    &format!("/api/projects/{project}/sourcemaps/{rel}/{bundle}"),
                    Some(Body::Text(text)),
                )?;
                if status == 200 {
                    ok.push(bundle.to_owned());
                } else {
                    failed.push(
                        serde_json::json!({ "file": p.display().to_string(), "error": v["error"] }),
                    );
                }
            }
            out(&serde_json::json!({ "release": rel, "uploaded": ok, "failed": failed }));
            Ok(failed.is_empty() && !maps.is_empty())
        }
        Command::Import { project, file } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
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
                "rejected_sample": rejected }),
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
                    return Err(format!("export of {day} failed ({status}): {v}"));
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
                } => c.call(
                    "PUT",
                    &format!("/api/projects/{project}/flags/{key}"),
                    Some(Body::Json(json!({
                        "key": key, "enabled": !off, "rollout": rollout, "roles": roles,
                        "accounts": accounts, "description": description,
                    }))),
                )?,
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

/// The wiring checklist an agent must get to all-green before handing over.
fn verify(c: &Client, project: &str) -> Result<bool, String> {
    let report = verify_report(c, project)?;
    let ok = report["ok"] == true;
    out(&report);
    Ok(ok)
}

/// The verify checklist as JSON (shared with the MCP tool).
pub(crate) fn verify_report(c: &Client, project: &str) -> Result<Value, String> {
    let n = |rows: Vec<Value>| rows.first().and_then(|r| r["n"].as_i64()).unwrap_or(0);
    let recent = "timestamp >= now() - INTERVAL '1 day'";
    let events = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM events WHERE {recent}"),
    )?);
    let identified = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM events WHERE {recent} AND event = '$identify'"),
    )?);
    let with_role = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM events WHERE {recent} AND role IS NOT NULL"),
    )?);
    let releases = n(c.sql(
        project,
        &format!("SELECT count(DISTINCT release) AS n FROM events WHERE {recent}"),
    )?);

    let bots = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM raw_events WHERE {recent} AND environment = 'bot'"),
    )?);

    let (map_status, map) = c.call("GET", &format!("/api/projects/{project}/feature-map"), None)?;
    let mut features = Vec::new();
    if map_status == 200
        && let Some(m) = map["yaml"]
            .as_str()
            .and_then(|y| openlens_featuremap::parse(y).map)
    {
        // Raw events carry `feature` only for data-feature/feature() uses;
        // route entry points are resolved here with the map, like the board does.
        let rows = c.sql(
            project,
            &format!(
                "SELECT feature, path, count(*) AS n FROM events WHERE {recent} \
                 AND (feature IS NOT NULL OR path IS NOT NULL) GROUP BY feature, path"
            ),
        )?;
        let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for r in &rows {
            let n = r["n"].as_i64().unwrap_or(0);
            let key = r["feature"].as_str().map(str::to_owned).or_else(|| {
                r["path"]
                    .as_str()
                    .and_then(|p| m.feature_for_path(p))
                    .map(str::to_owned)
            });
            if let Some(k) = key {
                *counts.entry(k).or_default() += n;
            }
        }
        for f in &m.features {
            features.push(json!({ "key": f.key, "events_last_day": counts.get(&f.key).copied().unwrap_or(0) }));
        }
    }
    let checks = json!([
        { "check": "events arrive", "ok": events > 0, "detail": events, "fix": "add the SDK snippet and load a page" },
        { "check": "users are identified", "ok": identified > 0, "detail": identified, "fix": "call openlens.identify(user.id, { role }) after login" },
        { "check": "roles are sent", "ok": with_role > 0, "detail": with_role, "fix": "pass the user's real role field to identify()" },
        { "check": "release is tagged", "ok": releases > 0, "detail": releases, "fix": "pass release (version or git SHA) to init or data-release" },
        { "check": "feature map pushed", "ok": map_status == 200, "detail": map["version"], "fix": "openlens-cli features push" },
    ]);
    let ok = checks
        .as_array()
        .is_some_and(|a| a.iter().take(3).all(|c| c["ok"] == true))
        && map_status == 200;
    Ok(json!({ "ok": ok, "checks": checks, "features": features,
        "robots_last_day": bots,
        "note": "features with 0 events have not been exercised yet; click through them once to confirm they fire" }))
}

/// The rule an app's own agents follow to keep the feature map current.
const RULES_SNIPPET: &str = "## Product analytics (OpenLens)

- The app's features, success events and roles are declared in `openlens.features.yaml`.
- When you add, rename or remove a user-facing feature, update that file in the same change:
  its `entry` (routes, `data-feature` attribute) and its `success` event, and add
  `data-feature=\"<key>\"` to the feature's main control.
- When you add or rename a user role, map its raw value under `roles:`.
- Run `openlens-cli features lint --src .` before committing, then `openlens-cli features push` on deploy.
- Never send names, emails or content to OpenLens: ids, roles, plans and event names only.";

/// Every date from `from` to `to`, inclusive, as YYYY-MM-DD (at most 366).
fn days_between(from: (i32, u32, u32), to: (i32, u32, u32)) -> Result<Vec<String>, String> {
    fn days_in(y: i32, m: u32) -> u32 {
        match m {
            2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }
    let (mut y, mut m, mut d) = from;
    let mut out = Vec::new();
    loop {
        if d > days_in(y, m) {
            return Err(format!("{y:04}-{m:02}-{d:02} does not exist"));
        }
        out.push(format!("{y:04}-{m:02}-{d:02}"));
        if (y, m, d) >= to {
            break;
        }
        if out.len() >= 366 {
            return Err("export at most 366 days at a time".into());
        }
        d += 1;
        if d > days_in(y, m) {
            d = 1;
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
    }
    Ok(out)
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
    use super::plain_http_ok;

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
