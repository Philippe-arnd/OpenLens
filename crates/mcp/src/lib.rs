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
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
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
}
