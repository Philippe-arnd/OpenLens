//! How the `events` view reads the typed columns (#449 D-037, #482 D-039).
//!
//! Since #449 each `$vital` is stored with its facts as columns
//! (`vital_name`, `vital_value`, …), and since #482 the error, acquisition
//! and friction facts too (`error_message`, `acq_channel`, `click_rage`,
//! …). Files written before have no such columns: they read as NULL, and
//! the view then takes the value from the `properties` text with the
//! expression the engine used before, so old and new files give the same
//! answers and nothing is rewritten.
//!
//! The text is read only on rows of the column's own event that were
//! written before the column: for the #449 columns, a `$vital` whose
//! `vital_name` is NULL (an old row, or one whose name was not valid, in
//! which the expressions find nothing either); for the #482 columns, a row
//! without `facts_version`, which every row written since carries. A new
//! row never runs a regular expression.

/// The event a #482 column belongs to, and the expression that recovers
/// it from `properties` on a row written before it.
fn fallback_482(column: &str) -> Option<(&'static str, String)> {
    let text =
        |k: &str, class: &str| format!("regexp_match(r.properties, '\"\\${k}\":\"({class})\"')[1]");
    // `"$<key>":"<up to 200 characters, up to the first quote>"`.
    let acquisition = |k: &str| text(k, "[^\"]{1,200}");
    Some(match column {
        "vital_rating" => ("$vital", text("rating", "[a-z]{1,12}")),
        "error_message" => ("$error", text("message", "(?:[^\"\\\\]|\\\\.)*")),
        "error_source" => ("$error", text("source", "[^\"]*")),
        "error_stack" => ("$error", text("stack", "(?:[^\"\\\\]|\\\\.)*")),
        "acq_channel" => ("$pageview", acquisition("channel")),
        "acq_utm_source" => ("$pageview", acquisition("utm_source")),
        "acq_utm_campaign" => ("$pageview", acquisition("utm_campaign")),
        "acq_ref_host" => ("$pageview", acquisition("ref_host")),
        "click_rage" => ("$click", "r.properties LIKE '%\"$rage\":true%'".to_owned()),
        _ => return None,
    })
}

/// The expression that recovers the #449 column `column` from
/// `properties` on a row written before it, or `None` for another column.
fn fallback_449(column: &str) -> Option<String> {
    let number = |k: &str| {
        format!("CAST(regexp_match(r.properties, '\"\\${k}\":([0-9.eE+-]+)')[1] AS DOUBLE)")
    };
    let text =
        |k: &str, class: &str| format!("regexp_match(r.properties, '\"\\${k}\":\"({class})\"')[1]");
    Some(match column {
        "vital_name" => text("name", "[A-Z]+"),
        "vital_value" => number("value"),
        "vital_device" => text("device", "phone|tablet|desktop"),
        "vital_el" => text("el", "[^\"\\\\]{1,120}"),
        "vital_kind" => format!(
            "coalesce({}, {})",
            text("el_type", "[a-z]{1,12}"),
            text("input", "[a-z]{1,12}")
        ),
        "vital_res" => text("res", "[^\"\\\\]{1,200}"),
        // The parts of LCP and INP: numbers. (`vital_rating` is a #482
        // column, taken before this.)
        c if openlens_storage::typed_columns().iter().any(|t| t == c) => {
            number(c.strip_prefix("vital_")?)
        }
        _ => return None,
    })
}

/// The view's expression for the stored column `column` (`r.<column>`
/// for an untyped one).
pub(crate) fn select(column: &str) -> String {
    if let Some((event, f)) = fallback_482(column) {
        return format!(
            "CASE WHEN r.facts_version IS NULL AND r.event = '{event}' THEN {f} \
             ELSE r.{column} END AS {column}"
        );
    }
    match fallback_449(column) {
        Some(f) => format!(
            "CASE WHEN r.event = '$vital' AND r.vital_name IS NULL THEN {f} \
             ELSE r.{column} END AS {column}"
        ),
        None => format!("r.{column}"),
    }
}
