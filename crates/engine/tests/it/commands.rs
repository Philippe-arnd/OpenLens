//! The command registry (#308, D-042): what MCP and the REST API will be
//! compared against.

use std::collections::HashSet;

use openlens_engine::commands::REGISTRY;

#[test]
fn command_names_are_unique_and_named_by_area() {
    let mut seen = HashSet::new();
    for c in REGISTRY {
        assert!(seen.insert(c.name), "`{}` is registered twice", c.name);
        let (area, verb) = c.name.split_once('.').unwrap_or_default();
        assert!(
            !area.is_empty() && !verb.is_empty(),
            "`{}` is not `area.verb`",
            c.name
        );
    }
}
