//! MCP tools for the public status page (#561): read and change its
//! settings (`status_page.get`, `status_page.set`), and read the page as
//! anyone sees it (`status_page.public`). Each tool calls the route of its
//! command (D-020). [`call`] answers the names it knows.

use openlens_mcp::ToolSpec;
use serde_json::{Map, Value, json};

use crate::{Body, Client};

fn s(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::to_owned)
}

/// A path segment, percent-encoded.
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

fn base(args: &Value) -> String {
    let p = s(args, "project").unwrap_or_else(|| "default".into());
    format!("/api/projects/{}/status-page", enc(&p))
}

fn send(c: &Client, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let (st, v) = c.call(method, path, body.map(Body::Json))?;
    if (200..300).contains(&st) {
        Ok(v)
    } else {
        Err(crate::api_error(st, &v))
    }
}

/// The tools of this module.
pub(crate) fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "status_page",
            description: "The public status page. Without `slug`: its settings (enabled, slug, components with their public names and response_time, path `/status/<slug>`, and the monitored environments that can be shown). With `slug`: the page as anyone reads it (`/status/<slug>.json`): state, components with 90 daily bars and availability over 24h/7d/30d/90d, announcements, planned maintenance and detected incidents.",
            input_schema: json!({"type":"object","properties":{
                "project": {"type": "string", "description": "Project id (default: default)"},
                "slug": {"type": "string", "description": "Read the public page with this slug instead"}
            }}),
        },
        ToolSpec {
            name: "status_page_set",
            description: "Change the public status page (only the fields given change): enabled (needs a slug), slug (3 to 40 of a-z, 0-9 and -, unique across projects), components ([{environment, name, response_time}]; production is always first, other environments must be monitored). Off by default.",
            input_schema: json!({"type":"object","properties":{
                "project": {"type": "string", "description": "Project id (default: default)"},
                "enabled": {"type": "boolean"},
                "slug": {"type": "string", "pattern": "^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$"},
                "components": {"type": "array", "maxItems": 21, "items": {"type": "object", "properties": {
                    "environment": {"type": "string"},
                    "name": {"type": "string", "maxLength": 60},
                    "response_time": {"type": "boolean"}
                }, "required": ["environment"]}}
            }}),
        },
    ]
}

/// Run the tool `name`, or `None` when it is not one of [`specs`].
pub(crate) fn call(c: &Client, name: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "status_page" => match s(args, "slug") {
            Some(slug) => {
                crate::status_page::slug_path(&slug).and_then(|p| send(c, "GET", &p, None))
            }
            None => send(c, "GET", &base(args), None),
        },
        "status_page_set" => {
            let body: Map<String, Value> = ["enabled", "slug", "components"]
                .iter()
                .filter_map(|k| args.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                .collect();
            send(c, "PUT", &base(args), Some(Value::Object(body)))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_slug_cannot_reach_another_route() {
        for bad in ["../api/users", "a/b", "a?x=1", "A%2F", ""] {
            assert!(crate::status_page::slug_path(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            crate::status_page::slug_path("acme").as_deref(),
            Ok("/status/acme.json")
        );
    }
}
