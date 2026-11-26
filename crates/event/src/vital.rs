//! The Web Vitals facts of a `$vital` event as typed values (#449, D-037).
//!
//! The query layer used to pull them out of the stored `properties` text
//! with one regular expression per fact and per row. They are now written
//! next to `properties` as their own nullable columns. Each rule below is
//! the regular expression it replaces, read on the parsed JSON: a value is
//! kept only when that expression would have matched it, so a file written
//! before the columns existed (read through the same expressions) and a
//! file written after give the same answers. `properties` is unchanged.

use serde_json::{Map, Value};

use crate::limits;
use crate::names::VITAL;

/// The parts of LCP (`ttfb` … `render_delay`) and INP (`input_delay` …
/// `presentation`) the SDK sends, as `$<part>` (#467). Each one is the
/// column `vital_<part>`.
pub const PARTS: [&str; 7] = [
    "ttfb",
    "load_delay",
    "load_time",
    "render_delay",
    "input_delay",
    "processing",
    "presentation",
];

/// The facts of one `$vital`. Built only when `$name` is valid: a row
/// without a name has no typed facts at all, and readers fall back to the
/// text for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vital {
    /// `$name`: `LCP`, `INP`, `CLS`, … (`[A-Z]+`).
    pub name: String,
    /// `$value`, a JSON number.
    pub value: Option<f64>,
    /// `$device`: `phone`, `tablet` or `desktop`.
    pub device: Option<String>,
    /// `$el`: the element's selector (1 to 120 characters).
    pub el: Option<String>,
    /// `$el_type`, else `$input` (`[a-z]{1,12}`).
    pub kind: Option<String>,
    /// `$res`: the resource's path (1 to 200 characters).
    pub res: Option<String>,
    /// The value of each of [`PARTS`], in that order.
    pub parts: [Option<f64>; 7],
}

/// The typed facts of an event named `event` with these stored
/// `properties`, or `None` when it is not a `$vital` with a valid `$name`.
/// Hostile input is bounded: text over the capture limit is not parsed.
pub fn vital(event: &str, properties: &str) -> Option<Vital> {
    if event != VITAL || properties.len() > limits::MAX_PROPERTIES_BYTES {
        return None;
    }
    let props: Map<String, Value> = serde_json::from_str(properties).ok()?;
    from_map(&props)
}

/// The facts of a `$vital` whose properties are already parsed.
pub(crate) fn from_map(props: &Map<String, Value>) -> Option<Vital> {
    let name = text(props, "name", 1, usize::MAX, |c| c.is_ascii_uppercase())?;
    let lower = |c: char| c.is_ascii_lowercase();
    let mut parts = [None; 7];
    for (slot, part) in parts.iter_mut().zip(PARTS) {
        *slot = number(props, part);
    }
    Some(Vital {
        name,
        value: number(props, "value"),
        device: text(props, "device", 1, usize::MAX, lower)
            .filter(|d| ["phone", "tablet", "desktop"].contains(&d.as_str())),
        el: text(props, "el", 1, 120, plain),
        kind: text(props, "el_type", 1, 12, lower).or_else(|| text(props, "input", 1, 12, lower)),
        res: text(props, "res", 1, 200, plain),
        parts,
    })
}

/// A character the JSON text holds as itself: not `"`, `\` or below
/// U+0020 (those are escaped, which `[^"\\]` does not match).
pub(crate) fn plain(c: char) -> bool {
    c != '"' && c != '\\' && c >= ' '
}

/// `"$<key>":"(…)"`: a string of `min..=max` characters, each allowed.
pub(crate) fn text(
    props: &Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
    ok: impl Fn(char) -> bool,
) -> Option<String> {
    let s = props.get(&format!("${key}"))?.as_str()?;
    let mut n = 0usize;
    for c in s.chars() {
        n += 1;
        if n > max || !ok(c) {
            return None;
        }
    }
    (n >= min).then(|| s.to_owned())
}

/// `"$<key>":([0-9.eE+-]+)` cast to a double: a JSON number.
fn number(props: &Map<String, Value>, key: &str) -> Option<f64> {
    props
        .get(&format!("${key}"))?
        .as_f64()
        .filter(|v| v.is_finite())
}
