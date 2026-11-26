//! How the `events` view reads the typed columns (#449, D-037).
//!
//! Since #449 each `$vital` is stored with its facts as columns
//! (`vital_name`, `vital_value`, …). Files written before have no such
//! columns: they read as NULL, and the view then takes the value from the
//! `properties` text with the regular expression the engine used before,
//! so old and new files give the same answers and nothing is rewritten.
//!
//! The text is read only for a `$vital` whose `vital_name` is NULL: an old
//! row, or a row whose name was not valid (the expressions find nothing in
//! it either). A new row with a name never runs a regular expression.

/// The expression that recovers `column` from `properties` on a row
/// written before the typed columns, or `None` for an untyped column.
fn fallback(column: &str) -> Option<String> {
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
        // The parts of LCP and INP: numbers.
        c if openlens_storage::typed_columns().iter().any(|t| t == c) => {
            number(c.strip_prefix("vital_")?)
        }
        _ => return None,
    })
}

/// The view's expression for the stored column `column` (`r.<column>`
/// for an untyped one).
pub(crate) fn select(column: &str) -> String {
    match fallback(column) {
        Some(f) => format!(
            "CASE WHEN r.event = '$vital' AND r.vital_name IS NULL THEN {f} \
             ELSE r.{column} END AS {column}"
        ),
        None => format!("r.{column}"),
    }
}
