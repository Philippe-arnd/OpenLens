//! MCP tools for status announcements and incident updates (#562): an
//! agent posts "deploying v2.3" from CI and resolves it after. Each tool
//! calls the REST route of an `announcements.*` command (D-020). Listed
//! after the other tools; [`call`] answers the names it knows.

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
    format!("/api/projects/{}/announcements", enc(&p))
}

fn id(args: &Value) -> Result<i64, String> {
    args.get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| "id (an announcement id from `announcements`) is required".into())
}

fn send(c: &Client, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let (st, v) = c.call(method, path, body.map(Body::Json))?;
    if (200..300).contains(&st) {
        Ok(v)
    } else {
        Err(crate::api_error(st, &v))
    }
}

/// `args` restricted to `keys`.
fn only(args: &Value, keys: &[&str]) -> Value {
    let m: Map<String, Value> = keys
        .iter()
        .filter_map(|k| args.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    Value::Object(m)
}

const FIELDS: &[&str] = &[
    "title",
    "body",
    "level",
    "components",
    "start",
    "end",
    "pinned",
    "incident",
];

/// The tools of this module.
pub(crate) fn specs() -> Vec<ToolSpec> {
    let with = |extra: Value| {
        let mut p = json!({ "project": { "type": "string", "description": "Project id (default: default)" } });
        if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        p
    };
    let fields = || {
        json!({
            "title": {"type": "string", "maxLength": 200},
            "body": {"type": "string", "maxLength": 4000, "description": "Plain text; markup is shown as text"},
            "level": {"type": "string", "enum": ["information", "degraded", "outage", "resolved"]},
            "components": {"type": "array", "items": {"type": "string"}, "description": "production or environment names"},
            "start": {"type": "string", "description": "RFC 3339, Unix seconds or YYYY-MM-DD HH:MM in the project's zone"},
            "end": {"type": "string"},
            "pinned": {"type": "boolean", "description": "Pin at the top (information only; unpins the others)"},
            "incident": {"type": "integer", "description": "The incident it follows (id from `announcements` offers)"}
        })
    };
    let states = json!({"type": "string", "enum": ["investigating", "identified", "monitoring", "resolved"]});
    let mut create = fields();
    if let Some(o) = create.as_object_mut() {
        o.insert("state".into(), states.clone());
    }
    let mut edit = fields();
    if let Some(o) = edit.as_object_mut() {
        o.insert("id".into(), json!({"type": "integer"}));
    }
    vec![
        ToolSpec {
            name: "announcements",
            description: "Status announcements of the last `days` (default 90): title, plain-text body, level, components, start and end, pinned, the incident followed, and dated updates oldest first; the pinned notice; offers on recent incidents (write_update: an open incident without words; mark_resolved: its incident closed, the announcement is not resolved yet); whether they go to the alert channels.",
            input_schema: json!({"type":"object","properties":with(json!({"days":{"type":"integer","minimum":1,"maximum":400}}))}),
        },
        ToolSpec {
            name: "announce",
            description: "Post a status announcement (\"Deploying v2.3\", \"Slower exports this afternoon\"). With `incident` and `state`, it follows that incident with a first update. Returns it with its id.",
            input_schema: json!({"type":"object","properties":with(create),"required":["title"]}),
        },
        ToolSpec {
            name: "announcement_update",
            description: "Post a dated update on an announcement: state investigating, identified, monitoring or resolved, and what is happening. A resolved update also resolves and ends it.",
            input_schema: json!({"type":"object","properties":with(json!({"id":{"type":"integer"},"state":states,"body":{"type":"string","maxLength":4000}})),"required":["id","state","body"]}),
        },
        ToolSpec {
            name: "announcement_edit",
            description: "Change an announcement: only the fields given change (end \"\" removes the end).",
            input_schema: json!({"type":"object","properties":with(edit),"required":["id"]}),
        },
        ToolSpec {
            name: "announcement_close",
            description: "End an announcement now (Mark resolved), with an optional closing body posted as a resolved update.",
            input_schema: json!({"type":"object","properties":with(json!({"id":{"type":"integer"},"body":{"type":"string"}})),"required":["id"]}),
        },
        ToolSpec {
            name: "announcement_delete",
            description: "Delete an announcement and its updates.",
            input_schema: json!({"type":"object","properties":with(json!({"id":{"type":"integer"}})),"required":["id"]}),
        },
        ToolSpec {
            name: "announcement_send",
            description: "Also send announcements, their updates and their end to the project's alert channels (email, webhook, ntfy): send true or false (off by default).",
            input_schema: json!({"type":"object","properties":with(json!({"send":{"type":"boolean"}})),"required":["send"]}),
        },
    ]
}

/// Run the tool `name`, or `None` when it is not one of [`specs`].
pub(crate) fn call(c: &Client, name: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "announcements" => {
            let q = args
                .get("days")
                .and_then(Value::as_i64)
                .map_or_else(String::new, |d| format!("?days={d}"));
            send(c, "GET", &format!("{}{q}", base(args)), None)
        }
        "announce" => {
            let mut keys = FIELDS.to_vec();
            keys.push("state");
            send(c, "POST", &base(args), Some(only(args, &keys)))
        }
        "announcement_update" => id(args).and_then(|i| {
            let path = format!("{}/{i}/updates", base(args));
            send(c, "POST", &path, Some(only(args, &["state", "body"])))
        }),
        "announcement_edit" => id(args).and_then(|i| {
            let path = format!("{}/{i}", base(args));
            send(c, "PATCH", &path, Some(only(args, FIELDS)))
        }),
        "announcement_close" => id(args).and_then(|i| {
            let path = format!("{}/{i}/close", base(args));
            send(c, "POST", &path, Some(only(args, &["body"])))
        }),
        "announcement_delete" => id(args).and_then(|i| {
            let path = format!("{}/{i}", base(args));
            send(c, "DELETE", &path, None)
        }),
        "announcement_send" => match args.get("send").and_then(Value::as_bool) {
            Some(on) => send(
                c,
                "PUT",
                &format!("{}/send", base(args)),
                Some(json!({ "send": on })),
            ),
            None => Err("send: true or false".into()),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_tool_is_answered_and_others_are_not() {
        let client = Client::new(Some("http://localhost:1".into()), Some("t".into())).unwrap();
        let args = json!({"id": 1, "title": "t", "state": "identified", "body": "b", "send": true});
        for t in specs() {
            assert!(
                call(&client, t.name, &args).is_some(),
                "tool `{}` is listed but not answered",
                t.name
            );
        }
        assert!(call(&client, "status", &args).is_none());
    }

    #[test]
    fn the_project_cannot_reach_another_route() {
        let p = base(&json!({"project": "a/../b?x=1"}));
        assert_eq!(p, "/api/projects/a%2F..%2Fb%3Fx%3D1/announcements");
    }
}
