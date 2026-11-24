//! Settings → Storage and backups (#218): the S3 destination (the
//! environment wins and is shown read-only; the secret is write-only and
//! sealed), its test and status, the backup plan, its history, *Back up
//! now* and the latest copy as a download. Admins only.

use axum::extract::{Form, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

async fn render(state: &AppState, notice: Option<String>, error: Option<String>) -> Response {
    let meta = &state.meta;
    let s = |k: &str| meta.setting(k).ok().flatten().filter(|v| !v.is_empty());
    let from_env = crate::s3::config().is_some();
    let env_cfg = crate::s3::config();
    let view = openlens_ui::StorageView {
        from_env,
        bucket: env_cfg
            .as_ref()
            .map(|c| c.bucket.clone())
            .or_else(|| s("s3:bucket")),
        region: env_cfg
            .as_ref()
            .map(|c| c.region.clone())
            .or_else(|| s("s3:region")),
        endpoint: env_cfg
            .as_ref()
            .and_then(|c| c.endpoint.clone())
            .or_else(|| s("s3:endpoint")),
        prefix: env_cfg
            .as_ref()
            .map(|c| c.prefix.trim_end_matches('/').to_owned())
            .or_else(|| s("s3:prefix")),
        access_key: env_cfg
            .as_ref()
            .map(|c| c.access_key.clone())
            .or_else(|| s("s3:access_key")),
        secret_set: from_env || s("s3:secret").is_some(),
        secret_ok: crate::s3::configured(meta, &state.data_dir).is_some(),
        last_ok: s("s3:last_ok"),
        last_objects: s("s3:last_objects"),
        last_error: s("s3:last_error"),
        plan: {
            let p = crate::backups::Plan::load(meta);
            openlens_ui::BackupPlan {
                schedule: p.schedule.clone(),
                hour: p.hour,
                weekday: p.weekday,
                keep: p.keep,
                dir: p.dir.clone(),
                folder: p.folder(&state.data_dir).display().to_string(),
                to_s3: p.to_s3,
            }
        },
        history: crate::backups::history(meta)
            .into_iter()
            .map(|r| openlens_ui::BackupRun {
                at: r.at,
                ok: r.ok,
                files: r.files,
                bytes: r.bytes,
                secs: r.secs,
                dest: r.dest,
                s3: r.s3,
                error: r.error,
                how: r.how,
            })
            .collect(),
        running: crate::backups::running(),
        has_latest: crate::backups::latest(state).is_some(),
        data_dir: state.data_dir.display().to_string(),
        notice,
        error,
    };
    html_page(StatusCode::OK, openlens_ui::storage(&view))
}

/// `GET /settings/storage`.
pub(crate) async fn page(_: crate::access::Admin, State(state): State<AppState>) -> Response {
    render(&state, None, None).await
}

#[derive(Deserialize)]
pub(crate) struct S3Form {
    #[serde(default)]
    action: String,
    #[serde(default)]
    bucket: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    access_key: String,
    /// Empty keeps the stored one.
    #[serde(default)]
    secret: String,
}

/// `POST /settings/storage/s3`: `save`, `test` or `remove`.
pub(crate) async fn s3(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<S3Form>,
) -> Response {
    if crate::s3::config().is_some() && f.action != "test" {
        return render(
            &state,
            None,
            Some("The S3 settings come from the environment; change them there.".into()),
        )
        .await;
    }
    match f.action.as_str() {
        "remove" => {
            for k in crate::s3::KEYS {
                let _ = state.meta.set_setting(&format!("s3:{k}"), "");
            }
            crate::privacy::audit(&state, &headers, None, "storage.s3_remove", "");
            render(
                &state,
                Some("S3 destination removed. Local data is untouched.".into()),
                None,
            )
            .await
        }
        "test" => {
            let (meta, dir) = (state.meta.clone(), state.data_dir.clone());
            let res = tokio::task::spawn_blocking(move || {
                let (cfg, _) =
                    crate::s3::configured(&meta, &dir).ok_or("no S3 destination is set")?;
                crate::s3::test(&cfg)
            })
            .await;
            match res {
                Ok(Ok(())) => {
                    render(
                        &state,
                        Some(
                            "Connection works: a probe object was written, read back and deleted."
                                .into(),
                        ),
                        None,
                    )
                    .await
                }
                Ok(Err(e)) => render(&state, None, Some(format!("Test failed: {e}"))).await,
                Err(_) => render(&state, None, Some("Test failed.".into())).await,
            }
        }
        _ => {
            let bucket = f.bucket.trim();
            let endpoint = f.endpoint.trim().trim_end_matches('/');
            if bucket.is_empty() || f.access_key.trim().is_empty() {
                return render(
                    &state,
                    None,
                    Some("Bucket and access key are required.".into()),
                )
                .await;
            }
            if !bucket
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
            {
                return render(
                    &state,
                    None,
                    Some("The bucket name has characters S3 does not accept.".into()),
                )
                .await;
            }
            if !endpoint.is_empty()
                && !endpoint.starts_with("https://")
                && !endpoint.starts_with("http://")
            {
                return render(
                    &state,
                    None,
                    Some(
                        "The endpoint starts with https:// (or http:// on a private network)."
                            .into(),
                    ),
                )
                .await;
            }
            let had_secret = state
                .meta
                .setting("s3:secret")
                .ok()
                .flatten()
                .is_some_and(|s| !s.is_empty());
            if f.secret.is_empty() && !had_secret {
                return render(&state, None, Some("Enter the secret key.".into())).await;
            }
            let m = &state.meta;
            let _ = m.set_setting("s3:bucket", bucket);
            let _ = m.set_setting("s3:region", f.region.trim());
            let _ = m.set_setting("s3:endpoint", endpoint);
            let _ = m.set_setting("s3:prefix", f.prefix.trim().trim_matches('/'));
            let _ = m.set_setting("s3:access_key", f.access_key.trim());
            if !f.secret.is_empty() {
                match crate::secrets::seal(&state.data_dir, f.secret.trim()) {
                    Ok(sealed) => {
                        let _ = m.set_setting("s3:secret", &sealed);
                    }
                    Err(e) => return render(&state, None, Some(e)).await,
                }
            }
            crate::privacy::audit(&state, &headers, None, "storage.s3_save", bucket);
            render(&state, Some("Saved. Use *Test the connection* to check it; the next copy runs within the hour.".into()), None).await
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PlanForm {
    schedule: String,
    hour: u8,
    #[serde(default)]
    weekday: Option<u8>,
    keep: usize,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    to_s3: Option<String>,
}

/// `POST /settings/storage/backups`: the plan.
pub(crate) async fn plan(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<PlanForm>,
) -> Response {
    let p = crate::backups::Plan {
        schedule: f.schedule,
        hour: f.hour,
        weekday: f.weekday.unwrap_or(1),
        keep: f.keep,
        dir: f.dir.trim().to_owned(),
        to_s3: f.to_s3.as_deref() == Some("on"),
    };
    if let Err(e) = p.validate() {
        return render(&state, None, Some(e)).await;
    }
    match serde_json::to_string(&p) {
        Ok(j) => {
            let _ = state.meta.set_setting("backup:plan", &j);
            crate::privacy::audit(&state, &headers, None, "backup.plan", &p.schedule);
            render(&state, Some("Backup plan saved.".into()), None).await
        }
        Err(e) => render(&state, None, Some(e.to_string())).await,
    }
}

/// `POST /settings/storage/backups/run`: back up now (in the background).
pub(crate) async fn run_now(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if crate::backups::running() {
        return render(&state, None, Some("A backup is already running.".into())).await;
    }
    let st = state.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || crate::backups::run(&st, "manual")).await;
    });
    crate::privacy::audit(&state, &headers, None, "backup.run", "");
    Redirect::to("/settings/storage").into_response()
}

/// `GET /settings/storage/backups/latest.tar.gz`.
pub(crate) async fn latest(_: crate::access::Admin, State(state): State<AppState>) -> Response {
    let st = state.clone();
    let made = tokio::task::spawn_blocking(move || {
        let dir = crate::backups::latest(&st).ok_or("no local backup yet")?;
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let out =
            std::env::temp_dir().join(format!("openlens-{name}-{}.tar.gz", std::process::id()));
        crate::backups::archive(&dir, &out)?;
        Ok::<_, String>((out, name))
    })
    .await;
    let Ok(Ok((out, name))) = made else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("No backup", "There is no local backup to download yet."),
        );
    };
    let file = match tokio::fs::File::open(&out).await {
        Ok(f) => f,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Unlinked now; the open handle keeps streaming it.
    let _ = std::fs::remove_file(&out);
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/gzip"),
    );
    if let Ok(v) =
        HeaderValue::from_str(&format!("attachment; filename=\"openlens-{name}.tar.gz\""))
    {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}
