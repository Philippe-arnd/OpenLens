//! Minimal MCP server: JSON-RPC 2.0 over stdio, tools only.
//!
//! Implemented from the public Model Context Protocol specification
//! (`initialize`, `tools/list`, `tools/call`, `ping`), with no SDK
//! dependency. Tool logic lives in the caller ([`Tools`]).
//!
//! Layer: L4 (see `docs/architecture.md`).

use std::io::{BufRead, Read, Write};

use serde_json::{Value, json};

/// Protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Largest accepted request line, in bytes.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// Largest tool result text, in bytes: one call must not flood the
/// agent's context (#687). Fixed, not a setting (D-014).
pub const MAX_RESULT_BYTES: usize = 50 * 1024;

const TRUNCATED_HINT: &str = "add LIMIT or narrow the period / filters";

/// A tool description for `tools/list`.
pub struct ToolSpec {
    /// Tool name.
    pub name: &'static str,
    /// What it does, for the agent.
    pub description: &'static str,
    /// JSON Schema of the arguments.
    pub input_schema: Value,
}

/// The tools a server exposes.
pub trait Tools {
    /// All tools.
    fn list(&self) -> Vec<ToolSpec>;
    /// Run a tool. `Err` is reported to the agent as a tool error (not a
    /// protocol error), so it can read the message and fix its call.
    fn call(&self, name: &str, args: &Value) -> Result<Value, String>;
}

fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Handle one request; `None` for notifications.
pub fn handle(tools: &dyn Tools, name: &str, version: &str, req: &Value) -> Option<Value> {
    let id = req.get("id")?.clone();
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(match method {
        "initialize" => response(
            &id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": name, "version": version },
                "instructions": "OpenLens: product analytics for PMs. Start with `guide` to wire an app, `verify` to check it, `board` for the KPIs.",
            }),
        ),
        "ping" => response(&id, json!({})),
        "tools/list" => response(
            &id,
            json!({ "tools": tools.list().iter().map(|t| json!({
                "name": t.name, "description": t.description, "inputSchema": t.input_schema
            })).collect::<Vec<_>>() }),
        ),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_default();
            let tool = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !tools.list().iter().any(|t| t.name == tool) {
                return Some(error(&id, -32602, &format!("unknown tool `{tool}`")));
            }
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (text, is_error) = match tools.call(tool, &args) {
                Ok(v) => (bounded_text(v), false),
                Err(e) => (e, true),
            };
            response(
                &id,
                json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
            )
        }
        _ => error(&id, -32601, &format!("method `{method}` not found")),
    })
}

/// A tool result as compact JSON of at most [`MAX_RESULT_BYTES`]. A
/// larger result keeps the leading items of its largest top-level array
/// (or of the result itself, when it is an array, under `rows`) that fit,
/// and says so with `truncated`, `shown`, `total` and a `hint`. Any other
/// shape that is too large is replaced by a note. The text is always
/// valid JSON.
fn bounded_text(v: Value) -> String {
    let text = serde_json::to_string(&v).unwrap_or_default();
    if text.len() <= MAX_RESULT_BYTES {
        return text;
    }
    let size = text.len();
    let mut obj = match v {
        Value::Array(items) => {
            let mut obj = serde_json::Map::new();
            obj.insert("rows".into(), Value::Array(items));
            obj
        }
        Value::Object(obj) => obj,
        _ => return too_large(size),
    };
    let Some(key) = obj
        .iter()
        .filter_map(|(k, v)| v.as_array().map(|a| (k, a.len())))
        .max_by_key(|(_, n)| *n)
        .map(|(k, _)| k.clone())
    else {
        return too_large(size);
    };
    let Some(Value::Array(mut items)) = obj.insert(key.clone(), json!([])) else {
        return too_large(size);
    };
    // A tool that already cut its rows knows the larger total.
    let total = obj
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(u64::try_from(items.len()).unwrap_or(u64::MAX));
    obj.insert("truncated".into(), Value::Bool(true));
    obj.insert("shown".into(), json!(items.len()));
    obj.insert("total".into(), json!(total));
    obj.insert("hint".into(), json!(TRUNCATED_HINT));
    // The frame already counts `shown` at its widest, so it only shrinks.
    let mut used = serde_json::to_string(&obj).map_or(usize::MAX, |t| t.len());
    let mut shown = 0;
    for item in &items {
        let len = serde_json::to_string(item).map_or(usize::MAX, |t| t.len());
        let next = used.saturating_add(len).saturating_add(1);
        if next > MAX_RESULT_BYTES {
            break;
        }
        used = next;
        shown += 1;
    }
    items.truncate(shown);
    obj.insert("shown".into(), json!(shown));
    obj.insert(key, Value::Array(items));
    let text = serde_json::to_string(&obj).unwrap_or_default();
    if text.len() <= MAX_RESULT_BYTES {
        text
    } else {
        too_large(size)
    }
}

fn too_large(bytes: usize) -> String {
    json!({
        "truncated": true,
        "error": format!("result too large ({bytes} bytes, limit {MAX_RESULT_BYTES})"),
        "hint": TRUNCATED_HINT,
    })
    .to_string()
}

/// Serve until stdin closes. One JSON-RPC message per line.
pub fn serve(tools: &dyn Tools, name: &str, version: &str) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut line = String::new();
    let mut reader = stdin.lock();
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take(MAX_LINE as u64 + 1)
            .read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if line.len() > MAX_LINE {
            writeln!(out, "{}", error(&Value::Null, -32600, "message too large"))?;
            // Drain the rest of the oversized line.
            let mut sink = Vec::new();
            reader.read_until(b'\n', &mut sink)?;
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(tools, name, version, &req),
            Err(_) => Some(error(&Value::Null, -32700, "parse error")),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Tools for Echo {
        fn list(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "echo",
                description: "echo",
                input_schema: json!({"type": "object"}),
            }]
        }
        fn call(&self, _: &str, args: &Value) -> Result<Value, String> {
            if args.get("fail").is_some() {
                Err("nope".into())
            } else {
                Ok(args.clone())
            }
        }
    }

    #[test]
    fn protocol_round_trip() {
        let init = handle(
            &Echo,
            "t",
            "0",
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(
            handle(
                &Echo,
                "t",
                "0",
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .is_none()
        );
        let list = handle(
            &Echo,
            "t",
            "0",
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "echo");
        let ok = handle(&Echo, "t", "0", &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"a":1}}})).unwrap();
        assert_eq!(ok["result"]["isError"], false);
        let failed = handle(&Echo, "t", "0", &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"fail":1}}})).unwrap();
        assert_eq!(
            failed["result"]["isError"], true,
            "tool errors are results the agent can read"
        );
        let unknown = handle(
            &Echo,
            "t",
            "0",
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"x"}}),
        )
        .unwrap();
        assert!(unknown["error"].is_object());
        let missing = handle(
            &Echo,
            "t",
            "0",
            &json!({"jsonrpc":"2.0","id":6,"method":"nope"}),
        )
        .unwrap();
        assert_eq!(missing["error"]["code"], -32601);
    }

    fn call_text(args: Value) -> String {
        let reply = handle(&Echo, "t", "0", &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":args}})).unwrap();
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_huge_result_is_cut_to_valid_json_that_says_so() {
        let rows: Vec<Value> = (0..10_000)
            .map(|i| json!({"event": "page_view", "path": format!("/p/{i}"), "n": i}))
            .collect();
        let args = json!({ "rows": rows });
        assert!(args.to_string().len() > MAX_RESULT_BYTES);
        let text = call_text(args);
        assert!(text.len() <= MAX_RESULT_BYTES);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["truncated"], true);
        let shown = v["rows"].as_array().unwrap().len();
        assert!(shown > 0 && shown < 10_000);
        assert_eq!(v["shown"], shown);
        assert_eq!(v["total"], 10_000);
        assert_eq!(v["rows"][0]["n"], 0, "the leading rows are kept");
        assert!(v["hint"].as_str().unwrap().contains("LIMIT"));

        // Too large without any array to cut: still valid JSON that says so.
        let text = call_text(json!({ "blob": "x".repeat(MAX_RESULT_BYTES * 2) }));
        assert!(text.len() <= MAX_RESULT_BYTES);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["truncated"], true);
    }

    #[test]
    fn a_small_result_is_unchanged_but_compact() {
        let args = json!({ "rows": [{"a": 1}, {"b": [1, 2]}], "note": "x" });
        let text = call_text(args.clone());
        assert!(!text.contains('\n'), "compact, not pretty-printed");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), args);
    }
}
