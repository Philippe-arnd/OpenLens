//! The team page (#216) and each viewer's profile (#215). `/users/*` is
//! admin-only (the access layer); `/me` is for anyone signed in.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

fn projects_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `active`, `invited` (never signed in, link open), `expired` (never
/// signed in, no open link) or `disabled`, and the open link's expiry.
fn status(m: &openlens_meta::Meta, u: &openlens_meta::User) -> (&'static str, Option<String>) {
    if u.disabled {
        return ("disabled", None);
    }
    let pending = m.pending_link(&u.id).ok().flatten();
    let expires = pending
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string());
    match (&u.last_sign_in, pending) {
        (Some(_), _) => ("active", None),
        (None, Some(_)) => ("invited", expires),
        (None, None) => ("expired", None),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    done: Option<String>,
}

/// `GET /users?q=`.
pub(crate) async fn page(
    _: crate::access::Admin,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Response {
    let notice = match q.done.as_deref() {
        Some("removed") => Some("Removed. Their sessions ended."),
        _ => None,
    };
    render(&state, None, &q.q, notice).await
}

async fn render(
    state: &AppState,
    link: Option<(String, String)>,
    q: &str,
    notice: Option<&str>,
) -> Response {
    let meta = state.meta.clone();
    let needle = q.trim().to_lowercase();
    let res = tokio::task::spawn_blocking(move || {
        let users = meta.users()?;
        let ids: Vec<String> = meta.projects()?.into_iter().map(|p| p.id).collect();
        let rows: Vec<(openlens_meta::User, &'static str, Option<String>)> = users
            .into_iter()
            .filter(|u| {
                needle.is_empty()
                    || u.email.contains(&needle)
                    || u.name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&needle))
            })
            .map(|u| {
                let (s, e) = status(&meta, &u);
                (u, s, e)
            })
            .collect();
        Ok::<_, openlens_meta::MetaError>((rows, ids))
    })
    .await;
    let Ok(Ok((rows, ids))) = res else {
        return html_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Oops", "The metadata store is unavailable."),
        );
    };
    let view: Vec<openlens_ui::UserRow<'_>> = rows
        .iter()
        .map(|(u, s, e)| openlens_ui::UserRow {
            user: u,
            status: s,
            expires: e.clone(),
        })
        .collect();
    html_page(
        StatusCode::OK,
        openlens_ui::users(
            &view,
            &ids,
            link.as_ref().map(|(e, l)| (e.as_str(), l.as_str())),
            q,
            notice,
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct InviteForm {
    email: String,
    #[serde(default)]
    name: String,
    role: String,
    #[serde(default)]
    projects: String,
}

/// `POST /users`: invite (or update) someone and show their login link.
pub(crate) async fn invite(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<InviteForm>,
) -> Response {
    let projects = projects_list(&f.projects);
    let meta = state.meta.clone();
    let res = tokio::task::spawn_blocking(move || {
        let u = meta.upsert_user(
            &f.email,
            &f.role,
            (!projects.is_empty()).then_some(&projects[..]),
        )?;
        if !f.name.trim().is_empty() {
            meta.update_user(
                &u.id,
                Some(&f.name),
                &u.email,
                &u.role,
                u.projects.as_deref(),
            )?;
        }
        let t = meta.create_login_link_for(Some(&u.id), 7 * 24 * 3600)?;
        Ok::<_, openlens_meta::MetaError>((u.email, t))
    })
    .await;
    match res {
        Ok(Ok((email, t))) => {
            let link = format!("{}/l/{t}", crate::public_url(&headers));
            crate::privacy::audit(&state, &headers, None, "user.invite", &email);
            render(&state, Some((email, link)), "", None).await
        }
        Ok(Err(e)) => oops(&e.to_string()),
        Err(_) => oops("Try again."),
    }
}

async fn load(state: &AppState, id: &str) -> Option<openlens_meta::User> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.user(&id))
        .await
        .ok()?
        .ok()?
}

/// `GET /users/{id}`.
pub(crate) async fn edit_page(
    _: crate::access::Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(u) = load(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such user."),
        );
    };
    let (s, _) = status(&state.meta, &u);
    let ids: Vec<String> = state
        .meta
        .projects()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.id)
        .collect();
    html_page(StatusCode::OK, openlens_ui::user_edit(&u, s, &ids))
}

#[derive(Deserialize)]
pub(crate) struct UpdateForm {
    #[serde(default)]
    name: String,
    email: String,
    role: String,
    #[serde(default)]
    projects: String,
}

/// `POST /users/{id}`: name, email, role and projects.
pub(crate) async fn update(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<UpdateForm>,
) -> Response {
    let projects = projects_list(&f.projects);
    let meta = state.meta.clone();
    let pid = id.clone();
    let res = tokio::task::spawn_blocking(move || {
        meta.update_user(
            &pid,
            Some(&f.name),
            &f.email,
            &f.role,
            (!projects.is_empty()).then_some(&projects[..]),
        )
    })
    .await;
    match res {
        Ok(Ok(Some(u))) => {
            crate::privacy::audit(
                &state,
                &headers,
                None,
                "user.update",
                &format!("{} ({})", u.email, u.role),
            );
            Redirect::to(&format!("/users/{id}")).into_response()
        }
        Ok(Ok(None)) => oops("No such user."),
        Ok(Err(e)) => oops(&e.to_string()),
        Err(_) => oops("Try again."),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct ConfirmForm {
    #[serde(default)]
    confirm: String,
}

/// `POST /users/{id}/{link|revoke|disable|enable|delete}`.
pub(crate) async fn action(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, action)): Path<(String, String)>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    let Some(u) = load(&state, &id).await else {
        return oops("No such user.");
    };
    let meta = state.meta.clone();
    let back = format!("/users/{id}");
    match action.as_str() {
        "link" => {
            if u.disabled {
                return oops("Enable sign-in first.");
            }
            let pid = id.clone();
            match tokio::task::spawn_blocking(move || {
                meta.create_login_link_for(Some(&pid), 7 * 24 * 3600)
            })
            .await
            {
                Ok(Ok(t)) => {
                    let link = format!("{}/l/{t}", crate::public_url(&headers));
                    render(&state, Some((u.email, link)), "", None).await
                }
                _ => oops("Try again."),
            }
        }
        "revoke" | "disable" | "enable" | "delete" => {
            if action == "delete" && f.confirm.trim().to_lowercase() != u.email {
                return oops("Type the user's email exactly to remove them.");
            }
            let act = action.clone();
            let res = tokio::task::spawn_blocking(move || match act.as_str() {
                "revoke" => meta.revoke_links(&id).map(|_| true),
                "disable" => meta.set_user_disabled(&id, true),
                "enable" => meta.set_user_disabled(&id, false),
                _ => meta.delete_user(&id),
            })
            .await;
            match res {
                Ok(Ok(_)) if action == "delete" => {
                    crate::privacy::audit(&state, &headers, None, "user.delete", &u.email);
                    Redirect::to("/users?done=removed").into_response()
                }
                Ok(Ok(_)) => {
                    crate::privacy::audit(
                        &state,
                        &headers,
                        None,
                        &format!("user.{action}"),
                        &u.email,
                    );
                    Redirect::to(&back).into_response()
                }
                Ok(Err(e)) => oops(&e.to_string()),
                Err(_) => oops("Try again."),
            }
        }
        _ => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such action."),
        ),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct MeQuery {
    #[serde(default)]
    done: Option<String>,
}

/// `GET /me`.
pub(crate) async fn me_page(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Query(q): Query<MeQuery>,
) -> Response {
    let user = if v.id == auth::OWNER {
        None
    } else {
        load(&state, &v.id).await
    };
    let notice = q.done.is_some().then_some("Saved.");
    html_page(
        StatusCode::OK,
        openlens_ui::me(
            user.as_ref().and_then(|u| u.name.as_deref()),
            user.as_ref().map(|u| u.email.as_str()),
            v.tz.as_deref(),
            notice,
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct MeForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    timezone: String,
}

/// `POST /me`: my time zone (and name).
pub(crate) async fn me_save(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Form(f): Form<MeForm>,
) -> Response {
    let tz = f.timezone.trim().to_owned();
    if !tz.is_empty() && crate::api::check_timezone(&tz).is_err() {
        return oops("The time zone must be an IANA name like Europe/Paris.");
    }
    let meta = state.meta.clone();
    let res = tokio::task::spawn_blocking(move || {
        if v.id == auth::OWNER {
            return meta.set_setting("owner_timezone", &tz);
        }
        let tzo = (!tz.is_empty()).then_some(tz.as_str());
        meta.set_user_timezone(&v.id, tzo)?;
        if let Some(u) = meta.user(&v.id)? {
            meta.update_user(
                &u.id,
                Some(&f.name),
                &u.email,
                &u.role,
                u.projects.as_deref(),
            )?;
        }
        Ok(())
    })
    .await;
    match res {
        Ok(Ok(())) => Redirect::to("/me?done=1").into_response(),
        Ok(Err(e)) => oops(&e.to_string()),
        Err(_) => oops("Try again."),
    }
}
