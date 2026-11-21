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
                description: "Create a project; returns the capture key and the snippet to add to the app.",
                input_schema: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
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
                name: "verify",
                description: "Wiring checklist: events arrive, users identified with a role, release tagged, map pushed, events per feature. Each failing check carries its fix.",
                input_schema: json!({"type":"object","properties":proj}),
            },
            ToolSpec {
                name: "board",
                description: "The Focus board numbers (KPIs by role/environment/release, features, roles, role gaps, releases, vitals).",
                input_schema: json!({"type":"object","properties":with(json!({"env":{"type":"string"},"role":{"type":"string"},"release":{"type":"string"}}))}),
            },
            ToolSpec {
                name: "sql",
                description: "Read-only SQL (DataFusion) on table `events`: uuid, event, distinct_id, session_id, timestamp, url, path, feature, role, account, plan, environment, release, properties (JSON text), date. Prune with `date >= 'YYYY-MM-DD'`. Max 10k rows.",
                input_schema: json!({"type":"object","properties":with(json!({"query":{"type":"string"}})),"required":["query"]}),
            },
            ToolSpec {
                name: "login_link",
                description: "A one-time login link (7 days) to send to the PM.",
                input_schema: json!({"type":"object","properties":{}}),
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
                let (st, v) = c.call(
                    "POST",
                    "/api/projects",
                    Some(Body::Json(json!({ "name": name }))),
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
                        json!({
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
            "sql" => {
                let query = s(args, "query").ok_or("query is required")?;
                Ok(json!({ "rows": c.sql(&project(args), &query)? }))
            }
            "login_link" => {
                let (st, v) = c.call("POST", "/api/login-links", None)?;
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
