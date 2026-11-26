//! One small client for every LLM mode (#244): `anthropic` (Messages API,
//! with prompt caching) and `openai-compatible` (Chat Completions: OpenAI,
//! a LiteLLM proxy, OpenRouter, Azure OpenAI, Mistral, vLLM, Ollama). Both
//! with tools. Off unless configured: env vars win over Settings → AI.
//!
//! The key is only read here and sent in a header to the provider; it is
//! never logged, never put in a request body we keep, never returned.

use std::time::Duration;

use serde_json::{Value, json};

use crate::AppState;

/// Which API the provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    /// Anthropic Messages API.
    Anthropic,
    /// `POST {base}/chat/completions`.
    OpenAi,
}

impl Provider {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai-compatible",
        }
    }
}

/// The configured model access.
#[derive(Clone)]
pub(crate) struct Config {
    pub provider: Provider,
    pub base_url: String,
    key: Option<String>,
    pub model: String,
    /// For short lookups (routing); the main model when unset.
    pub fast_model: String,
    /// Where it comes from: `env` or `settings`.
    pub source: &'static str,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("key", &self.key.as_ref().map(|_| "<set>"))
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Is a key set (always true is not required for local servers)?
    pub(crate) fn has_key(&self) -> bool {
        self.key.is_some()
    }
}

const ANTHROPIC: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-opus-5-5";

fn env(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// The model access, if any: env vars first (the existing
/// `OPENLENS_LLM_API_KEY` alone keeps meaning Anthropic), else Settings → AI.
pub(crate) fn config(state: &AppState) -> Option<Config> {
    let from_env = env("OPENLENS_LLM_PROVIDER")
        .or_else(|| env("OPENLENS_LLM_API_KEY").map(|_| "anthropic".into()));
    if let Some(p) = from_env {
        return build(
            &p,
            env("OPENLENS_LLM_BASE_URL"),
            env("OPENLENS_LLM_API_KEY"),
            env("OPENLENS_LLM_MODEL"),
            env("OPENLENS_LLM_FAST_MODEL"),
            "env",
        );
    }
    let s = |k: &str| {
        state
            .meta
            .setting(k)
            .ok()
            .flatten()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let key = s("ai_key").and_then(|sealed| crate::secrets::open(&state.data_dir, &sealed).ok());
    build(
        &s("ai_provider")?,
        s("ai_base_url"),
        key,
        s("ai_model"),
        s("ai_fast_model"),
        "settings",
    )
}

fn build(
    provider: &str,
    base: Option<String>,
    key: Option<String>,
    model: Option<String>,
    fast: Option<String>,
    source: &'static str,
) -> Option<Config> {
    let provider = match provider {
        "anthropic" => Provider::Anthropic,
        "openai-compatible" | "openai" | "litellm" | "ollama" => Provider::OpenAi,
        _ => return None,
    };
    let base_url = match provider {
        Provider::Anthropic => base.unwrap_or_else(|| ANTHROPIC.into()),
        Provider::OpenAi => base?,
    };
    if provider == Provider::Anthropic && key.is_none() {
        return None;
    }
    let model = match (provider, model) {
        (_, Some(m)) => m,
        (Provider::Anthropic, None) => DEFAULT_MODEL.into(),
        (Provider::OpenAi, None) => return None,
    };
    Some(Config {
        provider,
        base_url: base_url.trim_end_matches('/').to_owned(),
        key,
        fast_model: fast.unwrap_or_else(|| model.clone()),
        model,
        source,
    })
}

/// A tool the model may call.
#[derive(Debug, Clone)]
pub(crate) struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// One message of the conversation, provider-neutral.
#[derive(Debug, Clone)]
pub(crate) enum Msg {
    User(String),
    Assistant {
        text: String,
        calls: Vec<Call>,
    },
    /// Results of the previous calls: (call id, content).
    Results(Vec<(String, String)>),
}

/// A tool call the model made.
#[derive(Debug, Clone)]
pub(crate) struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// The model's reply.
#[derive(Debug, Clone, Default)]
pub(crate) struct Reply {
    pub text: String,
    pub calls: Vec<Call>,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
}

/// The request body for a provider (also what "What is sent" shows).
pub(crate) fn body(
    cfg: &Config,
    model: &str,
    system: &str,
    brief: &str,
    msgs: &[Msg],
    tools: &[Tool],
) -> Value {
    match cfg.provider {
        Provider::Anthropic => {
            let n = tools.len();
            let tools: Vec<Value> = tools
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut v = json!({ "name": t.name, "description": t.description, "input_schema": t.schema });
                    if i + 1 == n {
                        v["cache_control"] = json!({ "type": "ephemeral" });
                    }
                    v
                })
                .collect();
            let messages: Vec<Value> = msgs
                .iter()
                .map(|m| match m {
                    Msg::User(t) => json!({ "role": "user", "content": t }),
                    Msg::Assistant { text, calls } => {
                        let mut c: Vec<Value> = Vec::new();
                        if !text.is_empty() {
                            c.push(json!({ "type": "text", "text": text }));
                        }
                        for k in calls {
                            c.push(json!({ "type": "tool_use", "id": k.id, "name": k.name, "input": k.args }));
                        }
                        json!({ "role": "assistant", "content": c })
                    }
                    Msg::Results(rs) => json!({
                        "role": "user",
                        "content": rs.iter().map(|(id, c)| json!({ "type": "tool_result", "tool_use_id": id, "content": c })).collect::<Vec<_>>()
                    }),
                })
                .collect();
            json!({
                "model": model,
                "max_tokens": 2000,
                "system": [
                    { "type": "text", "text": system },
                    { "type": "text", "text": brief, "cache_control": { "type": "ephemeral" } }
                ],
                "tools": tools,
                "messages": messages,
            })
        }
        Provider::OpenAi => {
            let mut messages =
                vec![json!({ "role": "system", "content": format!("{system}\n\n{brief}") })];
            for m in msgs {
                match m {
                    Msg::User(t) => messages.push(json!({ "role": "user", "content": t })),
                    Msg::Assistant { text, calls } => {
                        let mut v = json!({ "role": "assistant", "content": text });
                        if !calls.is_empty() {
                            v["tool_calls"] = calls
                                .iter()
                                .map(|k| json!({ "id": k.id, "type": "function", "function": { "name": k.name, "arguments": k.args.to_string() } }))
                                .collect();
                        }
                        messages.push(v);
                    }
                    Msg::Results(rs) => {
                        for (id, c) in rs {
                            messages
                                .push(json!({ "role": "tool", "tool_call_id": id, "content": c }));
                        }
                    }
                }
            }
            json!({
                "model": model,
                "max_tokens": 2000,
                "messages": messages,
                "tools": tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })).collect::<Vec<_>>(),
            })
        }
    }
}

/// Send a request body (blocking: run it on `spawn_blocking`).
pub(crate) fn send(cfg: &Config, body: &Value) -> Result<Reply, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .into();
    let (url, req) = match cfg.provider {
        Provider::Anthropic => {
            let url = format!("{}/v1/messages", cfg.base_url);
            let r = agent
                .post(&url)
                .header("x-api-key", cfg.key.as_deref().unwrap_or(""))
                .header("anthropic-version", "2026-10-21")
                .header("content-type", "application/json");
            (url, r)
        }
        Provider::OpenAi => {
            let url = format!("{}/chat/completions", cfg.base_url);
            let mut r = agent.post(&url).header("content-type", "application/json");
            if let Some(k) = &cfg.key {
                r = r.header("authorization", &format!("Bearer {k}"));
            }
            (url, r)
        }
    };
    let mut resp = req.send_json(body).map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let v: Value = resp
        .body_mut()
        .with_config()
        .limit(4 << 20)
        .read_json()
        .map_err(|e| format!("{url}: unreadable answer ({e})"))?;
    if status >= 400 {
        let msg = v["error"]["message"]
            .as_str()
            .or_else(|| v["error"].as_str())
            .unwrap_or("error");
        return Err(format!("the provider answered {status}: {msg}"));
    }
    Ok(parse(cfg.provider, &v))
}

/// Read a provider's answer.
pub(crate) fn parse(p: Provider, v: &Value) -> Reply {
    let mut r = Reply::default();
    match p {
        Provider::Anthropic => {
            for b in v["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => r.text.push_str(b["text"].as_str().unwrap_or("")),
                    Some("tool_use") => r.calls.push(Call {
                        id: b["id"].as_str().unwrap_or("").to_owned(),
                        name: b["name"].as_str().unwrap_or("").to_owned(),
                        args: b["input"].clone(),
                    }),
                    _ => {}
                }
            }
            let u = &v["usage"];
            r.cached_tokens = u["cache_read_input_tokens"].as_i64().unwrap_or(0);
            r.input_tokens = u["input_tokens"].as_i64().unwrap_or(0)
                + r.cached_tokens
                + u["cache_creation_input_tokens"].as_i64().unwrap_or(0);
            r.output_tokens = u["output_tokens"].as_i64().unwrap_or(0);
        }
        Provider::OpenAi => {
            let m = &v["choices"][0]["message"];
            r.text = m["content"].as_str().unwrap_or("").to_owned();
            for (i, c) in m["tool_calls"].as_array().into_iter().flatten().enumerate() {
                let args = c["function"]["arguments"]
                    .as_str()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| c["function"]["arguments"].clone());
                r.calls.push(Call {
                    id: c["id"]
                        .as_str()
                        .map_or_else(|| format!("call_{i}"), str::to_owned),
                    name: c["function"]["name"].as_str().unwrap_or("").to_owned(),
                    args,
                });
            }
            let u = &v["usage"];
            r.input_tokens = u["prompt_tokens"].as_i64().unwrap_or(0);
            r.cached_tokens = u["prompt_tokens_details"]["cached_tokens"]
                .as_i64()
                .unwrap_or(0);
            r.output_tokens = u["completion_tokens"].as_i64().unwrap_or(0);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn cfg(p: Provider) -> Config {
        Config {
            provider: p,
            base_url: "http://x".into(),
            key: Some("sk-secret".into()),
            model: "m".into(),
            fast_model: "m".into(),
            source: "env",
        }
    }

    #[test]
    fn bodies_cache_the_prefix_and_never_carry_the_key() {
        let tools = [Tool {
            name: "kpi",
            description: "d",
            schema: json!({"type":"object"}),
        }];
        let msgs = [
            Msg::User("why?".into()),
            Msg::Assistant {
                text: String::new(),
                calls: vec![Call {
                    id: "c1".into(),
                    name: "kpi".into(),
                    args: json!({}),
                }],
            },
            Msg::Results(vec![("c1".into(), "a\tb".into())]),
        ];
        let a = body(
            &cfg(Provider::Anthropic),
            "m",
            "rules",
            "brief",
            &msgs,
            &tools,
        );
        assert_eq!(a["system"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(a["tools"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(a["messages"][2]["content"][0]["tool_use_id"], "c1");
        let o = body(&cfg(Provider::OpenAi), "m", "rules", "brief", &msgs, &tools);
        assert_eq!(o["messages"][2]["tool_calls"][0]["function"]["name"], "kpi");
        assert_eq!(o["messages"][3]["role"], "tool");
        for b in [a, o] {
            assert!(!b.to_string().contains("sk-secret"));
        }
        assert!(!format!("{:?}", cfg(Provider::OpenAi)).contains("sk-secret"));
    }

    #[test]
    fn replies_are_read_with_their_usage() {
        let a = parse(
            Provider::Anthropic,
            &json!({"content":[{"type":"text","text":"Hi"},{"type":"tool_use","id":"t","name":"kpi","input":{"x":1}}],
                    "usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":5}}),
        );
        assert_eq!(
            (
                a.text.as_str(),
                a.calls.len(),
                a.input_tokens,
                a.cached_tokens
            ),
            ("Hi", 1, 100, 90)
        );
        let o = parse(
            Provider::OpenAi,
            &json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"q","function":{"name":"sql","arguments":"{\"query\":\"count users\"}"}}]}}],
                    "usage":{"prompt_tokens":50,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":40}}}),
        );
        assert_eq!(o.calls[0].args["query"], "count users");
        assert_eq!((o.input_tokens, o.cached_tokens), (50, 40));
        assert!(
            build("anthropic", None, None, None, None, "env").is_none(),
            "no key"
        );
        assert!(
            build(
                "openai-compatible",
                None,
                None,
                Some("m".into()),
                None,
                "env"
            )
            .is_none(),
            "no base URL"
        );
        let ollama = build(
            "ollama",
            Some("http://localhost:11434/v1/".into()),
            None,
            Some("llama3".into()),
            None,
            "env",
        )
        .unwrap();
        assert_eq!(
            (ollama.provider, ollama.base_url.as_str()),
            (Provider::OpenAi, "http://localhost:11434/v1")
        );
    }
}
