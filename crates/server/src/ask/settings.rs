//! The AI settings page. Moved out of ask.rs as is (#307).

use super::*;

pub(super) fn admin_guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    match auth::viewer(state, headers) {
        None => Some(Redirect::to("/login").into_response()),
        Some(v) if !v.admin => Some(html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Admins only", "AI settings are for admins."),
        )),
        _ => None,
    }
}

pub(super) fn ai_view(state: &AppState, notice: Option<String>, error: Option<String>) -> Response {
    let s = |k: &str| state.meta.setting(k).ok().flatten().unwrap_or_default();
    let cfg = llm::config(state);
    html_page(
        if error.is_some() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::OK
        },
        openlens_ui::ai_settings(&openlens_ui::AiView {
            from_env: cfg.as_ref().is_some_and(|c| c.source == "env"),
            active: cfg.as_ref().map(|c| {
                format!(
                    "{} · {} (fast: {})",
                    c.provider.name(),
                    c.model,
                    c.fast_model
                )
            }),
            provider: s("ai_provider"),
            base_url: s("ai_base_url"),
            model: s("ai_model"),
            fast_model: s("ai_fast_model"),
            key_set: !s("ai_key").is_empty() || cfg.as_ref().is_some_and(llm::Config::has_key),
            budget: budget(),
            notice: notice.as_deref(),
            error: error.as_deref(),
        }),
    )
}

/// `GET /settings/ai` (admins).
pub(crate) async fn ai_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = admin_guard(&state, &headers) {
        return r;
    }
    ai_view(&state, None, None)
}

#[derive(Deserialize)]
pub(crate) struct AiForm {
    #[serde(default)]
    action: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    fast_model: String,
    /// Empty keeps the stored key; write-only.
    #[serde(default)]
    key: String,
}

/// `POST /settings/ai` (admins): save, test, or forget the key.
pub(crate) async fn ai_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<AiForm>,
) -> Response {
    if let Some(r) = admin_guard(&state, &headers) {
        return r;
    }
    match f.action.as_str() {
        "test" => {
            let Some(cfg) = llm::config(&state) else {
                return ai_view(
                    &state,
                    None,
                    Some("Nothing to test: save a provider first.".into()),
                );
            };
            let body = llm::body(
                &cfg,
                &cfg.model,
                "Reply with the single word OK.",
                "",
                &[Msg::User("Test".into())],
                &[],
            );
            let c2 = cfg.clone();
            let t0 = Instant::now();
            match tokio::task::spawn_blocking(move || llm::send(&c2, &body)).await {
                Ok(Ok(r)) => ai_view(
                    &state,
                    Some(format!(
                        "It works: {} answered \"{}\" in {:.1} s ({} tokens).",
                        cfg.model,
                        r.text.trim().chars().take(40).collect::<String>(),
                        t0.elapsed().as_secs_f64(),
                        r.input_tokens + r.output_tokens
                    )),
                    None,
                ),
                Ok(Err(e)) => ai_view(&state, None, Some(format!("The test failed: {e}"))),
                Err(e) => ai_view(&state, None, Some(e.to_string())),
            }
        }
        "forget" => {
            let _ = state.meta.set_setting("ai_key", "");
            crate::privacy::audit(&state, &headers, None, "ai.key_removed", "");
            ai_view(&state, Some("The key is removed.".into()), None)
        }
        _ => {
            let provider = match f.provider.as_str() {
                "" | "off" => "",
                "anthropic" => "anthropic",
                "openai-compatible" => "openai-compatible",
                _ => return ai_view(&state, None, Some("Pick a provider.".into())),
            };
            let base = f.base_url.trim();
            if !base.is_empty() && !(base.starts_with("https://") || base.starts_with("http://")) {
                return ai_view(&state, None, Some("The base URL starts with https:// (or http:// for a server on this network).".into()));
            }
            if provider == "openai-compatible" && (base.is_empty() || f.model.trim().is_empty()) {
                return ai_view(
                    &state,
                    None,
                    Some("An OpenAI-compatible server needs its base URL and a model.".into()),
                );
            }
            for (k, v) in [
                ("ai_provider", provider),
                ("ai_base_url", base),
                ("ai_model", f.model.trim()),
                ("ai_fast_model", f.fast_model.trim()),
            ] {
                let v: String = v.chars().take(300).collect();
                let _ = state.meta.set_setting(k, &v);
            }
            if !f.key.trim().is_empty() {
                match crate::secrets::seal(&state.data_dir, f.key.trim()) {
                    Ok(sealed) => {
                        let _ = state.meta.set_setting("ai_key", &sealed);
                    }
                    Err(e) => return ai_view(&state, None, Some(e)),
                }
            }
            crate::privacy::audit(&state, &headers, None, "ai.settings", provider);
            ai_view(&state, Some("Saved.".into()), None)
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PrivacyForm {
    #[serde(default)]
    person_level: Option<String>,
}

/// `POST /p/{id}/ask/privacy` (admins): allow person-level detail or not.
pub(crate) async fn privacy(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<PrivacyForm>,
) -> Response {
    if let Some(r) = admin_guard(&state, &headers) {
        return r;
    }
    let on = f.person_level.is_some();
    let _ = state
        .meta
        .set_project_setting(&id, "ai_person_level", if on { "on" } else { "" });
    crate::privacy::audit(
        &state,
        &headers,
        Some(&id),
        "ai.person_level",
        if on { "on" } else { "off" },
    );
    Redirect::to(&format!("/p/{id}/ask")).into_response()
}
