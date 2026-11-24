//! Ask history, token budget and rate limit. Moved out of ask.rs as is (#307).

use super::*;

pub(super) fn history_key(user: &str) -> String {
    format!("ask:{}", crate::short_hash(user))
}

pub(super) fn history(state: &AppState, project: &str, user: &str) -> Vec<Turn> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let mut v: Vec<Turn> = state
        .meta
        .project_setting(project, &history_key(user))
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    v.retain(|t| t.at >= cutoff);
    v
}

pub(super) fn save_history(state: &AppState, project: &str, user: &str, turns: &[Turn]) {
    let start = turns.len().saturating_sub(KEEP_TURNS);
    if let Ok(j) = serde_json::to_string(&turns[start..]) {
        let _ = state
            .meta
            .set_project_setting(project, &history_key(user), &j);
    }
}

pub(super) fn budget() -> i64 {
    std::env::var("OPENLENS_LLM_DAILY_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DAILY_TOKENS)
}

/// (day, tokens used that day) for a project.
pub(super) fn usage(state: &AppState, project: &str) -> (String, i64) {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let (d, n): (String, i64) = state
        .meta
        .project_setting(project, "ask_usage")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    if d == today { (d, n) } else { (today, 0) }
}

pub(super) fn add_usage(state: &AppState, project: &str, tokens: i64) {
    let (d, n) = usage(state, project);
    if let Ok(j) = serde_json::to_string(&(d, n + tokens)) {
        let _ = state.meta.set_project_setting(project, "ask_usage", &j);
    }
}

pub(super) fn rate_ok(state: &AppState, user: &str) -> bool {
    state
        .caches
        .ask_rate
        .update(user.to_owned(), |v| {
            let mut v = v.unwrap_or_default();
            v.retain(|t| t.elapsed() < Duration::from_secs(3600));
            if v.len() >= PER_HOUR {
                return (Some(v), false);
            }
            v.push(Instant::now());
            (Some(v), true)
        })
        .unwrap_or(false)
}
