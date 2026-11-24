//! AI provider presets (#406): pick a provider, paste the key. The preset
//! fills the stored `ai_provider` / `ai_base_url` / `ai_model` /
//! `ai_fast_model` settings; the models come from the provider's own list
//! (`GET /models`), so no model id is hardcoded in the UI. The ids below are
//! only a fallback for a provider that does not list its models.

use std::time::Duration;

use serde_json::Value;

use crate::llm::Provider;

/// One provider preset.
#[derive(Debug)]
pub(crate) struct Preset {
    /// Stored as `ai_preset`, sent by the form.
    pub id: &'static str,
    pub label: &'static str,
    pub protocol: Provider,
    pub base: &'static str,
    /// What a key looks like, and where to get one.
    pub key_hint: &'static str,
    pub key_url: &'static str,
    /// Main model: the first pattern with a match wins (the greatest id
    /// among its matches, which is the newest for these naming schemes).
    pub main: &'static [&'static str],
    /// Fast model, same rule.
    pub fast: &'static [&'static str],
    /// Ids that are not chat models.
    pub skip: &'static [&'static str],
    /// Used when the list cannot be read.
    pub fallback: (&'static str, &'static str),
}

const NOT_CHAT: &[&str] = &[
    "embed", "tts", "whisper", "dall", "image", "audio", "realtime", "moderation", "transcribe",
    "search", "ocr", "aqa", "instruct",
];

/// Every preset; `custom` shows every field.
pub(crate) const PRESETS: &[Preset] = &[
    Preset {
        id: "anthropic",
        label: "Anthropic",
        protocol: Provider::Anthropic,
        base: "https://api.anthropic.com",
        key_hint: "sk-ant-…",
        key_url: "https://console.anthropic.com/settings/keys",
        main: &["opus", "sonnet"],
        fast: &["haiku", "sonnet"],
        skip: &[],
        fallback: ("claude-opus-5-5", "claude-haiku-5-5"),
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        protocol: Provider::OpenAi,
        base: "https://api.openai.com/v1",
        key_hint: "sk-…",
        key_url: "https://platform.openai.com/api-keys",
        main: &["gpt-5", "gpt-4.1", "gpt-4o"],
        fast: &["gpt-5-mini", "gpt-4.1-mini", "mini"],
        skip: NOT_CHAT,
        fallback: ("gpt-4.1", "gpt-4.1-mini"),
    },
    Preset {
        id: "google",
        label: "Google (Gemini)",
        protocol: Provider::OpenAi,
        base: "https://generativelanguage.googleapis.com/v1beta/openai",
        key_hint: "AIza…",
        key_url: "https://aistudio.google.com/apikey",
        main: &["pro"],
        fast: &["flash-lite", "flash"],
        skip: NOT_CHAT,
        fallback: ("gemini-2.5-pro", "gemini-2.5-flash"),
    },
    Preset {
        id: "mistral",
        label: "Mistral",
        protocol: Provider::OpenAi,
        base: "https://api.mistral.ai/v1",
        key_hint: "32 characters",
        key_url: "https://console.mistral.ai/api-keys",
        main: &["mistral-large-latest", "large"],
        fast: &["mistral-small-latest", "small"],
        skip: NOT_CHAT,
        fallback: ("mistral-large-latest", "mistral-small-latest"),
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        protocol: Provider::OpenAi,
        base: "https://openrouter.ai/api/v1",
        key_hint: "sk-or-…",
        key_url: "https://openrouter.ai/keys",
        main: &["anthropic/claude-opus", "openai/gpt-5"],
        fast: &["anthropic/claude-haiku", "openai/gpt-5-mini"],
        skip: NOT_CHAT,
        fallback: ("anthropic/claude-opus-5-5", "anthropic/claude-haiku-5-5"),
    },
    Preset {
        id: "ollama",
        label: "Ollama (local)",
        protocol: Provider::OpenAi,
        base: "http://ollama:11434/v1",
        key_hint: "no key",
        key_url: "https://ollama.com/library",
        main: &[],
        fast: &[],
        skip: NOT_CHAT,
        fallback: ("llama3.1", "llama3.1"),
    },
];

pub(crate) fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The preset that stored settings match (existing installs), else
/// `custom`. An `ai_preset` setting wins when present.
pub(crate) fn detect(stored_preset: &str, provider: &str, base: &str) -> &'static str {
    if stored_preset == "custom" {
        return "custom";
    }
    if let Some(p) = preset(stored_preset) {
        return p.id;
    }
    let base = base.trim_end_matches('/');
    match provider {
        "" => "",
        "anthropic" if base.is_empty() || base == "https://api.anthropic.com" => "anthropic",
        _ => PRESETS
            .iter()
            .find(|p| p.base == base && p.protocol.name() == provider)
            .map_or("custom", |p| p.id),
    }
}

/// Pick (main, fast) from a provider's model ids.
pub(crate) fn pick(p: &Preset, ids: &[String]) -> (String, String) {
    let chat: Vec<&String> = ids
        .iter()
        .filter(|id| !p.skip.iter().any(|s| id.contains(s)))
        .collect();
    let best = |patterns: &[&str], avoid_small: bool| -> Option<String> {
        for pat in patterns {
            let hit = chat
                .iter()
                .filter(|id| id.contains(pat))
                .filter(|id| {
                    !avoid_small || pat.contains("mini") || !(id.contains("mini") || id.contains("nano"))
                })
                .max();
            if let Some(h) = hit {
                return Some((*h).clone());
            }
        }
        None
    };
    let main = best(p.main, true)
        .or_else(|| chat.first().map(|s| (*s).clone()))
        .unwrap_or_else(|| p.fallback.0.to_owned());
    let fast = best(p.fast, false).unwrap_or_else(|| main.clone());
    (main, fast)
}

/// Read a `/models` answer: `{"data": [{"id": …}]}` (Gemini prefixes
/// its ids with `models/`).
pub(crate) fn model_ids(v: &Value) -> Vec<String> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .map(|id| id.strip_prefix("models/").unwrap_or(id).to_owned())
        .collect()
}

/// A provider error, in words.
pub(crate) fn explain(status: u16, msg: &str) -> String {
    let what = match status {
        401 | 403 => "the key is invalid or not allowed",
        402 => "the account has no credit left",
        404 => "this address has no model list (check the base URL)",
        429 => "the provider limits this key right now (rate or credit)",
        _ => "the provider refused",
    };
    format!("{what} (HTTP {status}: {msg})")
}

/// List the provider's models (blocking): this is also the key check.
pub(crate) fn list_models(
    protocol: Provider,
    base: &str,
    key: Option<&str>,
) -> Result<Vec<String>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let base = base.trim_end_matches('/');
    let req = match protocol {
        Provider::Anthropic => agent
            .get(&format!("{base}/v1/models?limit=100"))
            .header("x-api-key", key.unwrap_or(""))
            .header("anthropic-version", "2026-10-21"),
        Provider::OpenAi => {
            let r = agent.get(&format!("{base}/models"));
            match key {
                Some(k) => r.header("authorization", &format!("Bearer {k}")),
                None => r,
            }
        }
    };
    let mut resp = req
        .call()
        .map_err(|e| format!("the provider cannot be reached ({e})"))?;
    let status = resp.status().as_u16();
    let v: Value = resp
        .body_mut()
        .with_config()
        .limit(8 << 20)
        .read_json()
        .unwrap_or(Value::Null);
    if status >= 400 {
        let msg = v["error"]["message"]
            .as_str()
            .or_else(|| v["error"].as_str())
            .unwrap_or("error");
        return Err(explain(status, msg));
    }
    Ok(model_ids(&v))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_newest_capable_model_is_main_and_the_cheap_one_fast() {
        let a = preset("anthropic").unwrap();
        let got = pick(
            a,
            &ids(&["claude-haiku-5-5", "claude-opus-4-1", "claude-opus-5-5", "claude-sonnet-5-5"]),
        );
        assert_eq!(got, ("claude-opus-5-5".into(), "claude-haiku-5-5".into()));
        let o = preset("openai").unwrap();
        let got = pick(
            o,
            &ids(&["gpt-4.1", "gpt-4.1-mini", "text-embedding-3-large", "gpt-4o-realtime", "whisper-1"]),
        );
        assert_eq!(got, ("gpt-4.1".into(), "gpt-4.1-mini".into()));
        let g = preset("google").unwrap();
        let got = pick(g, &model_ids(&serde_json::json!({"data": [
            {"id": "models/gemini-2.5-pro"}, {"id": "models/gemini-2.5-flash"},
            {"id": "models/gemini-2.5-flash-lite"}, {"id": "models/text-embedding-004"}
        ]})));
        assert_eq!(got, ("gemini-2.5-pro".into(), "gemini-2.5-flash-lite".into()));
        // Nothing matches: the first listed, then the fallback.
        let l = preset("ollama").unwrap();
        assert_eq!(pick(l, &ids(&["qwen3"])), ("qwen3".into(), "qwen3".into()));
        assert_eq!(pick(l, &[]), ("llama3.1".into(), "llama3.1".into()));
    }

    #[test]
    fn stored_settings_map_to_their_preset_or_custom() {
        assert_eq!(detect("", "anthropic", ""), "anthropic");
        assert_eq!(detect("", "openai-compatible", "https://api.openai.com/v1/"), "openai");
        assert_eq!(detect("", "openai-compatible", "http://litellm:4000/v1"), "custom");
        assert_eq!(detect("custom", "anthropic", ""), "custom");
        assert_eq!(detect("mistral", "openai-compatible", "x"), "mistral");
        assert_eq!(detect("", "", ""), "");
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert!(explain(401, "bad").starts_with("the key is invalid"));
        assert!(explain(402, "x").contains("no credit"));
    }
}
