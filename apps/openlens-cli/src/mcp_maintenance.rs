//! MCP tools over the `maintenance.*` commands (#563, D-075): list a
//! project's planned maintenance windows, and schedule, extend, end or
//! cancel one. Like every tool, they call the REST API (D-020).

use openlens_mcp::ToolSpec;
use serde_json::{Value, json};

use crate::{Body, Client};

fn path(args: &Value, tail: &str) -> String {
    let p = args
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let p: String = p
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("/api/projects/{p}/maintenance{tail}")
}

fn call_api(c: &Client, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
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
            name: "maintenance",
            description: "Planned maintenance windows of a project: planned, running and ended within 30 days, with their id, dates (Unix seconds and in words), components and text. During a window its components show 'maintenance', their incidents are recorded but not announced, and the time is not counted against availability.",
            input_schema: json!({"type":"object","properties":{"project":{"type":"string","description":"Project id (default: default)"}}}),
        },
        ToolSpec {
            name: "maintenance_set",
            description: "Change planned maintenance. action=schedule: text (required), start (now by default, RFC 3339, or YYYY-MM-DD HH:MM in the project's zone), end or minutes (at most 4320), components (production by default, or environment names), announce (once on the alert channels). Open one right before a deploy and end it after. action=extend: id and minutes (added to its end) or end. action=end: id, ends a running window now; incidents still failing are announced from then. action=cancel: id, removes a window that has not started.",
            input_schema: json!({"type":"object","properties":{
                "project":{"type":"string","description":"Project id (default: default)"},
                "action":{"type":"string","enum":["schedule","extend","end","cancel"]},
                "id":{"type":"integer"},
                "text":{"type":"string","maxLength":500},
                "start":{"type":"string"},
                "end":{"type":"string"},
                "minutes":{"type":"integer","minimum":1,"maximum":4320},
                "components":{"type":"array","items":{"type":"string"},"maxItems":10},
                "announce":{"type":"boolean","default":false}
            },"required":["action"]}),
        },
    ]
}

fn maintenance_set(c: &Client, args: &Value) -> Result<Value, String> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    let id = || {
        args.get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "id is required (from the maintenance tool)".to_owned())
    };
    let pick = |keys: &[&str]| -> Value {
        Value::Object(
            keys.iter()
                .filter_map(|k| args.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                .collect(),
        )
    };
    match action {
        "schedule" => {
            let body = pick(&["text", "start", "end", "minutes", "components", "announce"]);
            call_api(c, "POST", &path(args, ""), Some(body))
        }
        "extend" => {
            let body = pick(&["minutes", "end"]);
            call_api(
                c,
                "POST",
                &path(args, &format!("/{}/extend", id()?)),
                Some(body),
            )
        }
        "end" => call_api(c, "POST", &path(args, &format!("/{}/end", id()?)), None),
        "cancel" => call_api(c, "DELETE", &path(args, &format!("/{}", id()?)), None),
        _ => Err("action is schedule, extend, end or cancel".into()),
    }
}

/// Run the tool `name`, or `None` when it is not one of [`specs`].
pub(crate) fn call(c: &Client, name: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "maintenance" => call_api(c, "GET", &path(args, ""), None),
        "maintenance_set" => maintenance_set(c, args),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_cannot_reach_another_path() {
        let p = path(&json!({"project": "../a?b"}), "/1/end");
        assert!(p.starts_with("/api/projects/..%2Fa%3Fb/maintenance"), "{p}");
    }
}
