//! `GET /p/{project}/go?q=…` (#541): the sidebar's Go to… field. A name
//! that points to one place opens it (303); several places, close names or
//! nothing render a short list. The adapter of `goto.lookup` (features and
//! people) and of the sidebar's own section names (`openlens_ui`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::commands::goto::{self, LookupInput, MAX_QUERY};
use openlens_engine::commands::{Caller, Command, Error};
use openlens_ui::GotoPlace;
use serde::Deserialize;

use super::{html, project};
use crate::AppState;

#[derive(Deserialize)]
pub(super) struct GoQuery {
    #[serde(default)]
    q: Option<String>,
}

/// One path segment, percent-encoded: everything but the unreserved
/// characters, so a feature key or a person id can neither leave its
/// segment nor make the `Location` header invalid.
fn segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub(super) async fn go(
    _: crate::access::Session,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(gq): Query<GoQuery>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let raw = gq.q.unwrap_or_default();
    // A name longer than any we keep is refused (400) before any lookup,
    // rather than cut: a cut name could open a page nobody asked for.
    if raw.len() > MAX_QUERY {
        return html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message(
                "Can't go there",
                &format!("Type at most {MAX_QUERY} characters."),
            ),
        );
    }
    let q = raw.trim();
    if q.is_empty() {
        return Redirect::to(&format!("/p/{}", p.id)).into_response();
    }
    let input = LookupInput {
        project: p.id.clone(),
        q: q.to_owned(),
    };
    let found = match goto::Lookup::run(&state, &Caller::default(), input).await {
        Ok(m) => m,
        Err(Error::NotFound) => {
            return html(
                StatusCode::NOT_FOUND,
                openlens_ui::message("Not found", "No such project."),
            );
        }
        Err(e) => {
            return html(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't go there", &e.to_string()),
            );
        }
    };
    let feature = |f: &goto::FeatureRef| GotoPlace {
        label: f.name.clone(),
        href: format!("/p/{}/features/{}", p.id, segment(&f.key)),
        kind: "Feature",
    };
    let (mut exact, mut close) = openlens_ui::goto_sections(&p.id, q);
    exact.extend(found.features.iter().map(feature));
    if let Some(who) = &found.person {
        exact.push(GotoPlace {
            label: who.clone(),
            href: format!("/p/{}/people/{}", p.id, segment(who)),
            kind: "Person",
        });
    }
    if let [one] = exact.as_slice() {
        return Redirect::to(&one.href).into_response();
    }
    let several = !exact.is_empty();
    let list = if several {
        exact
    } else {
        close.extend(found.partial.iter().map(feature));
        close
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html(
        StatusCode::OK,
        openlens_ui::goto_results(&info, q, &list, several),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_segment_cannot_leave_its_place() {
        let s = super::segment("a/b?c#d\r\n é");
        assert!(!s.contains(['/', '?', '#', '\r', '\n', ' ']));
        assert!(s.is_ascii());
    }
}
