//! `openlens-cli`: the agent toolkit (docs/agents/wire.md).
//!
//! Every command prints one JSON document on stdout, so an agent can parse
//! it, and exits non-zero when something needs fixing. The server and the
//! admin token come from `--server`/`--token` or `OPENLENS_SERVER` /
//! `OPENLENS_TOKEN` (no config file, D-014).

mod lint;

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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a project; prints its capture key and the snippet to add.
    Init {
        /// Project name (usually the app's name).
        #[arg(long)]
        name: String,
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
    /// Create a one-time login link for the PM (valid 7 days).
    LoginLink,
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

struct Client {
    server: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    fn new(server: Option<String>, token: Option<String>) -> Result<Self, String> {
        let server = server
            .ok_or("missing --server (or OPENLENS_SERVER)")?
            .trim_end_matches('/')
            .to_owned();
        if !(server.starts_with("https://")
            || server.starts_with("http://localhost")
            || server.starts_with("http://127."))
        {
            return Err(
                "the server must use https (plain http is only allowed for localhost)".into(),
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
        })
    }

    fn call(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", self.server);
        let auth = format!("Bearer {}", self.token);
        let resp = match (method, body) {
            ("GET", _) => self.agent.get(&url).header("authorization", &auth).call(),
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

    fn sql(&self, project: &str, query: &str) -> Result<Vec<Value>, String> {
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

enum Body {
    Json(Value),
    Text(String),
}

fn out(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn run(cli: Cli) -> Result<bool, String> {
    let client = || Client::new(cli.server.clone(), cli.token.clone());
    match &cli.command {
        Command::Init { name } => {
            let (status, v) = client()?.call(
                "POST",
                "/api/projects",
                Some(Body::Json(json!({ "name": name }))),
            )?;
            out(&v);
            Ok(status == 201)
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
        Command::LoginLink => {
            let (status, v) = client()?.call("POST", "/api/login-links", None)?;
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
            for root in [automation_read_root, automation_write_root] {
                if !root.is_dir() {
                    return Err(format!("{} is not a directory", root.display()));
                }
            }
            Err("the MCP server is not implemented yet (ROADMAP.md, M2)".into())
        }
    }
}

/// The wiring checklist an agent must get to all-green before handing over.
fn verify(c: &Client, project: &str) -> Result<bool, String> {
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
    out(&json!({ "ok": ok, "checks": checks, "features": features,
        "note": "features with 0 events have not been exercised yet; click through them once to confirm they fire" }));
    Ok(ok)
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
