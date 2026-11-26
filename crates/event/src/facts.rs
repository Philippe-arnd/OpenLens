//! Every typed fact of one stored event, read from its `properties` once
//! (#449 D-037 for Web Vitals, #482 D-039 for errors, acquisition and
//! churn's friction).
//!
//! The engine used to pull these out of the `properties` text with one
//! regular expression (or `LIKE`) per fact and per row. They are now
//! written next to `properties` as nullable columns. Each rule below is
//! the expression it replaces, read on the parsed JSON: a value is kept
//! only where that expression would have matched, and it is the text the
//! expression would have returned, so a file written before the columns
//! existed (read through the same expressions) and a file written after
//! give the same answers. Only top-level keys count, as the SDK and the
//! server write them. `properties` is unchanged.
//!
//! Some old expressions return the value as it is spelled in the JSON
//! text, escapes included (`\"`, `\n`, `\u0001`): those columns hold that
//! spelling too, which is exactly what serde_json writes for the parsed
//! string, since stored `properties` are serde_json's own output.

use serde_json::{Map, Value};

use crate::limits;
use crate::names::{CLICK, ERROR, PAGEVIEW, VITAL};
use crate::vital::{self, Vital};

/// The version of the facts a row was written with, stored on every row
/// written since #482 (`facts_version`). A row without it was written
/// before, and readers take the #482 facts from its text.
pub const VERSION: i32 = 2;

/// The acquisition keys a `$pageview` may carry, as `$<key>`.
pub const ACQUISITION: [&str; 4] = ["channel", "utm_source", "utm_campaign", "ref_host"];

/// The typed facts of one event. Each field is set only on the event it
/// belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// `$vital`: the Web Vitals facts (D-037).
    pub vital: Option<Vital>,
    /// `$vital`: `$rating` (`[a-z]{1,12}`).
    pub rating: Option<String>,
    /// `$error`: `$message`, as spelled in the JSON text.
    pub message: Option<String>,
    /// `$error`: `$source`, as spelled in the JSON text, up to its first
    /// `"` (what `"([^"]*)"` returns).
    pub source: Option<String>,
    /// `$error`: `$stack`, as spelled in the JSON text.
    pub stack: Option<String>,
    /// `$pageview`: each of [`ACQUISITION`], in that order, as spelled in
    /// the JSON text up to its first `"`, 1 to 200 characters (what
    /// `"([^"]{1,200})"` returns).
    pub acquisition: [Option<String>; 4],
    /// `$click`: whether `$rage` is `true`.
    pub rage: Option<bool>,
}

/// The typed facts of an event named `event` with these stored
/// `properties`. Hostile input is bounded: text over the capture limit is
/// not parsed, and an event with nothing to read is not parsed at all.
pub fn facts(event: &str, properties: &str) -> Facts {
    let mut f = Facts::default();
    if properties.len() > limits::MAX_PROPERTIES_BYTES {
        return f;
    }
    let parse = || serde_json::from_str::<Map<String, Value>>(properties).ok();
    match event {
        VITAL => {
            if let Some(p) = parse() {
                f.vital = vital::from_map(&p);
                f.rating = vital::text(&p, "rating", 1, 12, |c| c.is_ascii_lowercase());
            }
        }
        ERROR => {
            if let Some(p) = parse() {
                f.message = spelled(&p, "message");
                f.source = spelled(&p, "source").map(|s| to_quote(&s).to_owned());
                f.stack = spelled(&p, "stack");
            }
        }
        PAGEVIEW => {
            // Most page views carry none of the keys: no parse for them.
            if ACQUISITION
                .iter()
                .any(|k| properties.contains(&format!("\"${k}\"")))
                && let Some(p) = parse()
            {
                for (slot, key) in f.acquisition.iter_mut().zip(ACQUISITION) {
                    *slot = spelled(&p, key).and_then(|s| {
                        let head = to_quote(&s);
                        (1..=200)
                            .contains(&head.chars().count())
                            .then(|| head.to_owned())
                    });
                }
            }
        }
        CLICK => {
            // `LIKE '%"$rage":true%'`: the text is checked first, so most
            // clicks are not parsed.
            f.rage = Some(
                properties.contains(r#""$rage":true"#)
                    && parse().is_some_and(|p| p.get("$rage") == Some(&Value::Bool(true))),
            );
        }
        _ => {}
    }
    f
}

/// The string `$<key>` as spelled between its quotes in the JSON text
/// (what `"((?:[^"\\]|\\.)*)"` returns), or `None` when it is not a string.
fn spelled(props: &Map<String, Value>, key: &str) -> Option<String> {
    let s = props.get(&format!("${key}"))?.as_str()?;
    let quoted = serde_json::to_string(s).ok()?;
    quoted
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .map(str::to_owned)
}

/// `s` up to its first `"` (in a spelled string, the one of a `\"`).
fn to_quote(s: &str) -> &str {
    s.split_once('"').map_or(s, |(head, _)| head)
}
