//! The AI settings page. Moved out of ask.rs as is (#307).

use super::*;

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
            preset: crate::llm_presets::detect(&s("ai_preset"), &s("ai_provider"), &s("ai_base_url")).to_owned(),
            presets: crate::llm_presets::PRESETS
                .iter()
                .map(|p| openlens_ui::AiPreset {
                    id: p.id,
                    label: p.label,
                    key_hint: p.key_hint,
                    key_url: p.key_url,
                })
                .collect(),
            models: s("ai_models").lines().map(str::to_owned).collect(),
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
pub(crate) async fn ai_page(_: crate::access::Admin, State(state): State<AppState>) -> Response {
    ai_view(&state, None, None)
}

#[derive(Deserialize)]
pub(crate) struct AiForm {
    #[serde(default)]
    action: String,
    /// A preset id (#406), `custom` or `off`; empty: the older form.
    #[serde(default)]
    preset: String,
    /// The protocol, for `custom` (and the older form).
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
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<AiForm>,
) -> Response {
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
        _ if crate::llm_presets::preset(&f.preset).is_some() => save_preset(&state, &headers, f).await,
        _ => {
            if f.preset == "off" {
                let _ = state.meta.set_setting("ai_provider", "");
                let _ = state.meta.set_setting("ai_preset", "");
                crate::privacy::audit(&state, &headers, None, "ai.settings", "");
                return ai_view(&state, Some("Saved: AI is off.".into()), None);
            }
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
                ("ai_preset", if provider.is_empty() { "" } else { "custom" }),
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

/// Save a provider preset (#406): fill the protocol and address, check the
/// key by listing the models, and pick the main and fast models from that
/// list (the form's model fields, when filled, win).
async fn save_preset(state: &AppState, headers: &HeaderMap, f: AiForm) -> Response {
    let Some(p) = crate::llm_presets::preset(&f.preset) else {
        return ai_view(state, None, Some("Pick a provider.".into()));
    };
    let base = match f.base_url.trim() {
        "" => p.base.to_owned(),
        b if b.starts_with("https://") || b.starts_with("http://") => b.to_owned(),
        _ => return ai_view(state, None, Some("The base URL starts with https:// (or http:// for a server on this network).".into())),
    };
    let stored = state
        .meta
        .setting("ai_key")
        .ok()
        .flatten()
        .filter(|k| !k.is_empty())
        .and_then(|sealed| crate::secrets::open(&state.data_dir, &sealed).ok());
    let key = match f.key.trim() {
        "" => stored,
        k => Some(k.to_owned()),
    };
    if key.is_none() && p.id != "ollama" {
        return ai_view(state, None, Some(format!("Paste the {} API key ({}).", p.label, p.key_hint)));
    }
    let (protocol, b2, k2) = (p.protocol, base.clone(), key.clone());
    let listed = tokio::task::spawn_blocking(move || {
        crate::llm_presets::list_models(protocol, &b2, k2.as_deref())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let (ids, check) = match listed {
        Ok(ids) => (ids, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let (mut main, mut fast) = if ids.is_empty() {
        (p.fallback.0.to_owned(), p.fallback.1.to_owned())
    } else {
        crate::llm_presets::pick(p, &ids)
    };
    if !f.model.trim().is_empty() {
        main = f.model.trim().chars().take(120).collect();
    }
    if !f.fast_model.trim().is_empty() {
        fast = f.fast_model.trim().chars().take(120).collect();
    }
    let models: Vec<&str> = ids.iter().take(300).map(String::as_str).collect();
    let models = models.join("\n");
    for (k, v) in [
        ("ai_preset", p.id),
        ("ai_provider", p.protocol.name()),
        ("ai_base_url", base.as_str()),
        ("ai_model", main.as_str()),
        ("ai_fast_model", fast.as_str()),
        ("ai_models", models.as_str()),
    ] {
        let _ = state.meta.set_setting(k, v);
    }
    if !f.key.trim().is_empty() {
        match crate::secrets::seal(&state.data_dir, f.key.trim()) {
            Ok(sealed) => {
                let _ = state.meta.set_setting("ai_key", &sealed);
            }
            Err(e) => return ai_view(state, None, Some(e)),
        }
    }
    crate::privacy::audit(state, headers, None, "ai.settings", p.id);
    match check {
        None => ai_view(
            state,
            Some(format!("Connected: {}, model {main}, fast model {fast}.", p.label)),
            None,
        ),
        Some(e) => ai_view(
            state,
            None,
            Some(format!("Saved with the default models ({main}, {fast}), but the check failed: {e}.")),
        ),
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
