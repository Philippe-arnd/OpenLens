//! Input schema helpers of the MCP tools (#688): the parameters many
//! tools share, described once.

use serde_json::{Value, json};

/// The parameters many tools share, each described once (#688): its
/// format and what an omitted one means. A tool whose parameter of the
/// same name means something else describes it itself.
const SHARED: &[(&str, &str)] = &[
    (
        "project",
        "Project id, as `projects` lists them (default: default)",
    ),
    (
        "env",
        "Environment name, as `environments` lists them (default: production)",
    ),
    (
        "role",
        "A role as the app sends it with identify(), counted at event time (default: every role)",
    ),
    (
        "release",
        "A release name as the SDK sends it, as `releases` lists them (default: every release)",
    ),
    (
        "since",
        "A release name: the period starts when it came out and runs to now",
    ),
];

/// The `properties` of a tool's input schema: `project`, then `extra`,
/// with each shared parameter that `extra` leaves undescribed given its
/// description from [`SHARED`].
pub(crate) fn with(extra: Value) -> Value {
    let mut p = json!({ "project": { "type": "string" } });
    if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
        a.extend(b.clone());
    }
    if let Some(props) = p.as_object_mut() {
        for (key, text) in SHARED {
            if let Some(param) = props.get_mut(*key).and_then(Value::as_object_mut) {
                param.entry("description").or_insert_with(|| json!(text));
            }
        }
    }
    p
}
