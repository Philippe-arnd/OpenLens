//! Crate layering. A crate may depend only on crates in a strictly lower
//! layer. Register every new workspace crate here; an unregistered crate
//! fails the check so the table cannot drift.

/// Layer of each workspace crate. `None` marks test helpers, which only
/// dev-dependencies may use.
pub const LAYERS: &[(&str, Option<u8>)] = &[
    ("openlens-event", Some(0)),
    ("openlens-storage", Some(1)),
    ("openlens-meta", Some(1)),
    ("openlens-query", Some(2)),
    ("openlens-flags", Some(2)),
    ("openlens-replay", Some(2)),
    ("openlens-errors", Some(2)),
    ("openlens-heatmaps", Some(2)),
    ("openlens-wire", Some(2)),
    ("openlens-engine", Some(3)),
    ("openlens-server", Some(4)),
    ("openlens-mcp", Some(4)),
    ("openlens-ui", Some(4)),
    ("openlens", Some(5)),
    ("openlens-cli", Some(5)),
    ("openlens-testkit", None),
    ("xtask", None),
];

/// Look up a crate's layer. `Err` means the crate is not registered.
pub fn layer_of(name: &str) -> Result<Option<u8>, ()> {
    LAYERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, l)| *l)
        .ok_or(())
}
