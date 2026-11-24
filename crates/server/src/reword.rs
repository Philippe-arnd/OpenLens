//! Optional plain-language rewording of insight cards by an LLM (D-016).
//!
//! Off unless `OPENLENS_LLM_API_KEY` (an Anthropic API key) is set; the model
//! is `OPENLENS_LLM_MODEL` (default `claude-opus-5-5`). The board never
//! waits: it renders the original wording, a background task asks for the
//! rewording, and the result is cached for a day per set of cards. A
//! rewording that drops or invents a number is discarded, and the original
//! wording always stays (as the tooltip). Only the cards' own text is sent
//! (titles, evidence, next steps: feature and role names, counts), never
//! events or user data.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use sha2::Digest;

const API: &str = "https://api.anthropic.com/v1/messages";
pub(crate) const TTL: Duration = Duration::from_secs(24 * 3600);

/// Plain wording of one card.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Plain {
    pub title: String,
    pub next_step: String,
}

fn settings() -> Option<(String, String)> {
    let key = std::env::var("OPENLENS_LLM_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())?;
    let model = std::env::var("OPENLENS_LLM_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "claude-opus-5-5".into());
    Some((key, model))
}

fn cache_key(cards: &[openlens_engine::Insight]) -> String {
    let mut h = sha2::Sha256::new();
    for c in cards {
        h.update(c.title.as_bytes());
        h.update(b"\x1f");
        h.update(c.evidence.as_bytes());
        h.update(b"\x1f");
        h.update(c.next_step.as_bytes());
        h.update(b"\x1e");
    }
    hex::encode(h.finalize())
}

/// The plain wording for these cards if it is ready; otherwise starts
/// computing it in the background and returns `None` (render the originals).
pub(crate) fn plain(
    state: &crate::AppState,
    cards: &[openlens_engine::Insight],
) -> Option<Vec<Plain>> {
    let (api_key, model) = settings()?;
    if cards.is_empty() {
        return None;
    }
    let key = cache_key(cards);
    if let Some(v) = state.caches.reword.get(&key) {
        return v;
    }
    // Mark as in flight (None) so concurrent renders do not ask again.
    state.caches.reword.insert(key.clone(), None);
    let caches = state.caches.clone();
    let originals: Vec<(String, String, String)> = cards
        .iter()
        .map(|c| (c.title.clone(), c.evidence.clone(), c.next_step.clone()))
        .collect();
    tokio::task::spawn_blocking(move || {
        let result = match ask(&api_key, &model, &originals) {
            Ok(v) => Some(v),
            Err(err) => {
                tracing::warn!(%err, "card rewording failed; keeping the original wording");
                None
            }
        };
        caches.reword.insert(key, result);
    });
    None
}

const SYSTEM: &str = "You rewrite product-analytics insight cards for a busy product manager. \
For each card, write a plain, friendly title (at most 110 characters) and a concrete next step \
(at most 160 characters). Keep every number, percentage, feature name, role name and release name \
exactly as given; do not add facts, numbers or advice that the card does not support. \
Return the cards in the same order.";

/// Digit runs in a text ("72%" -> "72", "5.2×" -> "5", "2").
fn numbers(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Keep a rewording only if it carries the same numbers as the original.
pub(crate) fn faithful(original: &str, plain: &str) -> bool {
    let (mut a, mut b) = (numbers(original), numbers(plain));
    a.sort();
    b.sort();
    a == b
}

fn ask(
    api_key: &str,
    model: &str,
    cards: &[(String, String, String)],
) -> Result<Vec<Plain>, String> {
    let input: Vec<_> = cards
        .iter()
        .map(|(t, e, n)| json!({ "title": t, "evidence": e, "next_step": n }))
        .collect();
    let body = json!({
        "model": model,
        "max_tokens": 4000,
        "system": SYSTEM,
        "fallbacks": "default",
        "output_config": {
            "effort": "low",
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "cards": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": { "title": { "type": "string" }, "next_step": { "type": "string" } },
                                "required": ["title", "next_step"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["cards"],
                    "additionalProperties": false
                }
            }
        },
        "messages": [{ "role": "user", "content": json!({ "cards": input }).to_string() }]
    });
    let resp: serde_json::Value = ureq::post(API)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2026-10-21")
        .header("anthropic-beta", "server-side-fallback-2026-07-01")
        .header("content-type", "application/json")
        .config()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .send_json(&body)
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let stop = resp["stop_reason"].as_str().unwrap_or("");
    if stop != "end_turn" {
        return Err(format!("stop_reason {stop}"));
    }
    let text = resp["content"]
        .as_array()
        .and_then(|a| a.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .ok_or("no text in the response")?;
    #[derive(Deserialize)]
    struct Out {
        cards: Vec<Plain>,
    }
    let out: Out = serde_json::from_str(text).map_err(|e| format!("not the expected JSON: {e}"))?;
    if out.cards.len() != cards.len() {
        return Err("card count changed".into());
    }
    Ok(out
        .cards
        .into_iter()
        .zip(cards)
        .map(|(p, (t, _, n))| Plain {
            // Per field: fall back to the original when a number moved.
            title: if faithful(t, &p.title) && p.title.chars().count() <= 140 {
                p.title
            } else {
                t.clone()
            },
            next_step: if faithful(n, &p.next_step) && p.next_step.chars().count() <= 220 {
                p.next_step
            } else {
                n.clone()
            },
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewording_must_keep_the_numbers() {
        assert!(faithful(
            "72% of churned users were last seen in billing",
            "Most churned users (72%) left from billing"
        ));
        assert!(!faithful(
            "72% of churned users were last seen in billing",
            "Three quarters of churned users left from billing"
        ));
        assert!(!faithful(
            "Release v2: errors went from 3% to 33%",
            "Release v2: errors jumped from 3% to 35%"
        ));
        assert!(faithful(
            "Users who use export stay 5.2× more",
            "Export users stay 5.2 times longer"
        ));
    }

    #[test]
    fn off_without_a_key() {
        // SAFETY of the assumption: tests do not set OPENLENS_LLM_API_KEY.
        if std::env::var("OPENLENS_LLM_API_KEY").is_err() {
            // `plain` asks nothing without settings.
            assert!(settings().is_none());
        }
    }
}
