//! `$feedback` (#718, D-116): what a user says, next to what they do.
//!
//! One event per message, from the browser SDK's `openlens.feedback()`, a
//! server SDK or `POST /capture`: a free text (`$text`), optionally the
//! feature it is about (`$about`, a key of the feature map) and a rating
//! (`$rating`, 1 to 5). The plain names `text`, `feature` and `rating` are
//! accepted too, for a support tool that sends a plain event, and folded
//! into the `$` names (`$feature` as well). The feature stays a property,
//! not the `feature` column: a feedback is no use of the feature.
//!
//! The text is personal data: it is cut here, removed from the files after
//! [`TEXT_DAYS`] (`openlens_storage::trim_feedback`), and erased with the
//! person.

use serde_json::{Map, Value};

/// The free text.
pub const TEXT: &str = "$text";
/// The feature key it is about.
pub const ABOUT: &str = "$about";
/// The rating, 1 to 5.
pub const RATING: &str = "$rating";
/// Longest text kept, in characters (longer texts are cut).
pub const MAX_CHARS: usize = 1_000;
/// Days a feedback keeps its text; the event row stays, with its feature
/// and rating. Fixed (D-015, D-116).
pub const TEXT_DAYS: i64 = 90;
/// Longest feature key accepted, in bytes.
const MAX_KEY: usize = 100;

/// The text as kept: control characters removed (line breaks and tabs
/// become spaces), trimmed, and cut to [`MAX_CHARS`] characters.
#[must_use]
pub fn clean_text(s: &str) -> String {
    let flat: String = s
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some(' ')
            } else if c.is_control() {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let cut: String = flat.trim().chars().take(MAX_CHARS).collect();
    cut.trim_end().to_owned()
}

/// A rating from 1 to 5 (an integer, or a string holding one).
fn rating(v: &Value) -> Option<i64> {
    let n = match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }?;
    (1..=5).contains(&n).then_some(n)
}

/// A feature key as the map writes them: letters, digits, `-`, `_`, `.`.
fn key(v: &Value) -> Option<String> {
    let k = v.as_str()?.trim();
    let ok = !k.is_empty()
        && k.len() <= MAX_KEY
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    ok.then(|| k.to_owned())
}

/// The value of `name`, else of `plain`; both are removed.
fn take(props: &mut Map<String, Value>, name: &str, plain: &str) -> Option<Value> {
    let a = props.remove(name);
    let b = props.remove(plain);
    a.or(b)
}

/// Clean the properties of a `$feedback` event in place: the text cut and
/// cleaned, a valid rating and feature key kept, anything else in those
/// fields dropped. False when nothing is left to keep (no text and no
/// rating): the event is then refused.
pub fn clean(props: &mut Map<String, Value>) -> bool {
    let text = take(props, TEXT, "text")
        .as_ref()
        .and_then(Value::as_str)
        .map(clean_text)
        .filter(|t| !t.is_empty());
    let rated = take(props, RATING, "rating").as_ref().and_then(rating);
    // `$feature` would make it a use of the feature: it is the subject.
    let column = props.remove("$feature");
    let about = take(props, ABOUT, "feature")
        .or(column)
        .as_ref()
        .and_then(key);
    if let Some(t) = &text {
        props.insert(TEXT.into(), Value::String(t.clone()));
    }
    if let Some(r) = rated {
        props.insert(RATING.into(), Value::from(r));
    }
    if let Some(k) = about {
        props.insert(ABOUT.into(), Value::String(k));
    }
    text.is_some() || rated.is_some()
}
