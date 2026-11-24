//! The error body of the JSON API (#312): `{"code", "message"}` with a code
//! from a closed list, plus `error` (the message) for one release so older
//! clients keep working.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Every code the API answers with. Clients rely on these, never on texts.
pub(crate) const CODES: [&str; 10] = [
    "invalid_request",
    "unauthorized",
    "forbidden",
    "not_found",
    "conflict",
    "too_large",
    "rate_limited",
    "busy",
    "unavailable",
    "internal",
];

/// The code of a status.
pub(crate) fn code(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        413 => "too_large",
        429 => "rate_limited",
        503 => "unavailable",
        500..=599 => "internal",
        _ => "invalid_request",
    }
}

/// A JSON error with the code of `status`.
pub(crate) fn json(status: StatusCode, message: impl std::fmt::Display) -> Response {
    with_code(status, code(status), message)
}

/// A JSON error with an explicit code from [`CODES`].
pub(crate) fn with_code(
    status: StatusCode,
    code: &'static str,
    message: impl std::fmt::Display,
) -> Response {
    debug_assert!(CODES.contains(&code), "{code} is not in the closed list");
    let m = message.to_string();
    (
        status,
        Json(json!({ "code": code, "message": m, "error": m })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_maps_into_the_closed_list() {
        for s in 400..600u16 {
            let st = StatusCode::from_u16(s).unwrap();
            assert!(CODES.contains(&code(st)), "{s}");
        }
    }
}
