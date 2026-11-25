//! Settings in three scopes (#438): how pages look follows the person, the
//! instance settings have one entry, and *About* says what runs here.
//!
//! The theme and the folded sidebar are kept per account and copied into
//! the `ol_theme` / `ol_nav` cookies whenever a session starts, so the
//! theme layer keeps reading cookies only (no database access per page),
//! and the choice follows the person to every browser they sign in from.
//! The cookie stays the choice of the admin token, which has no account,
//! and of signed-out pages.

use axum::extract::{Form, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::html_page;
use crate::{AppState, auth};

const YEAR: i64 = 31_536_000;

/// The two display cookies for a theme and a sidebar choice.
pub(crate) fn display_cookies(theme: Option<&str>, nav_folded: bool) -> [String; 2] {
    let theme = match theme {
        Some("light") => "light",
        Some("dark") => "dark",
        _ => "auto",
    };
    let nav = if nav_folded { "collapsed" } else { "expanded" };
    [
        format!("ol_theme={theme}; Path=/; SameSite=Lax; Max-Age={YEAR}"),
        format!("ol_nav={nav}; Path=/; SameSite=Lax; Max-Age={YEAR}"),
    ]
}

/// The user a fresh session cookie in `r` was made for, if `r` sets one.
fn session_started(r: &Response) -> Option<String> {
    r.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            let value = c
                .strip_prefix(&format!("{}=", auth::HOST_COOKIE))
                .or_else(|| c.strip_prefix(&format!("{}=", auth::COOKIE)))?;
            let value = value.split(';').next()?;
            // `<exp>.<user>.<signature>`, made by this server just now.
            let (_, rest) = value.split_once('.')?;
            let (user, _) = rest.rsplit_once('.')?;
            (!user.is_empty()).then(|| user.to_owned())
        })
}

tokio::task_local! {
    /// Whether the reader of this request is an admin (#438): the sidebar
    /// shows them the instance settings.
    static ADMIN: bool;
}

/// For `openlens_ui`: the current request's [`ADMIN`].
pub(crate) fn current_admin() -> bool {
    ADMIN.try_with(|a| *a).unwrap_or(false)
}

/// Knows who reads (for the sidebar), and when a response starts a
/// session, adds the account's display cookies to it.
pub(crate) async fn display_layer(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let admin = auth::viewer(&state, req.headers()).is_some_and(|v| v.admin);
    let mut r = ADMIN.scope(admin, next.run(req)).await;
    if let Some(user) = session_started(&r).filter(|u| u != auth::OWNER) {
        let meta = state.meta.clone();
        let look = tokio::task::spawn_blocking(move || meta.user_display(&user))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        if let Some((theme, folded)) = look {
            for c in display_cookies(theme.as_deref(), folded) {
                if let Ok(v) = HeaderValue::from_str(&c) {
                    r.headers_mut().append(header::SET_COOKIE, v);
                }
            }
        }
    }
    r
}

/// `GET /settings`: the instance settings start at *Team*.
pub(crate) async fn instance_home(_: crate::access::Admin) -> Response {
    Redirect::to("/users").into_response()
}

/// `GET /settings/about`.
pub(crate) async fn about(_: crate::access::Admin) -> Response {
    html_page(StatusCode::OK, openlens_ui::about())
}

/// `GET /me/appearance`.
pub(crate) async fn appearance_page(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<Done>,
) -> Response {
    let owner = v.id == auth::OWNER;
    let (theme, folded) = if owner {
        (
            auth::cookie(&headers, "ol_theme").filter(|t| t == "light" || t == "dark"),
            auth::cookie(&headers, "ol_nav").as_deref() == Some("collapsed"),
        )
    } else {
        state
            .meta
            .user_display(&v.id)
            .ok()
            .flatten()
            .unwrap_or((None, false))
    };
    let user = if owner {
        None
    } else {
        state.meta.user(&v.id).ok().flatten()
    };
    html_page(
        StatusCode::OK,
        openlens_ui::me(
            openlens_ui::MeTab::Appearance,
            user.as_ref().and_then(|u| u.name.as_deref()),
            user.as_ref().map(|u| u.email.as_str()),
            v.tz.as_deref(),
            q.done.is_some().then_some("Saved."),
            &openlens_ui::MePassword {
                enabled: crate::password::enabled(),
                owner,
                has_password: false,
                error: false,
            },
            &openlens_ui::MeLook {
                theme: theme.as_deref(),
                nav_folded: folded,
                per_account: !owner,
            },
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct Done {
    done: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct AppearanceForm {
    #[serde(default)]
    theme: String,
    #[serde(default)]
    nav_folded: Option<String>,
}

/// `POST /me/appearance`: save for the account (the admin token: this
/// browser only) and apply at once through the cookies.
pub(crate) async fn appearance_save(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Form(f): Form<AppearanceForm>,
) -> Response {
    let theme = match f.theme.as_str() {
        "light" => Some("light"),
        "dark" => Some("dark"),
        _ => None,
    };
    let folded = f.nav_folded.is_some();
    if v.id != auth::OWNER {
        let meta = state.meta.clone();
        let id = v.id.clone();
        let saved = tokio::task::spawn_blocking(move || meta.set_user_display(&id, theme, folded))
            .await
            .ok()
            .and_then(Result::ok);
        if saved.is_none() {
            return html_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                openlens_ui::message("Oops", "Try again."),
            );
        }
    }
    let mut r = Redirect::to("/me/appearance?done=1").into_response();
    for c in display_cookies(theme, folded) {
        if let Ok(v) = HeaderValue::from_str(&c) {
            r.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_new_session_names_its_user_and_nothing_else_does() {
        let with = |c: &str| {
            let mut r = Redirect::to("/").into_response();
            r.headers_mut()
                .append(header::SET_COOKIE, HeaderValue::from_str(c).unwrap());
            session_started(&r)
        };
        assert_eq!(
            with("ol_session=1999999999.u-ann.example.sig; Path=/; HttpOnly").as_deref(),
            Some("u-ann.example")
        );
        assert_eq!(
            with("__Host-ol_session=1.owner.sig; Path=/; Secure").as_deref(),
            Some("owner")
        );
        assert_eq!(with("ol_session=; Path=/; Max-Age=0"), None);
        assert_eq!(with("ol_theme=dark; Path=/"), None);
        assert_eq!(
            display_cookies(Some("evil; Domain=x"), true)[0],
            format!("ol_theme=auto; Path=/; SameSite=Lax; Max-Age={YEAR}")
        );
    }
}
