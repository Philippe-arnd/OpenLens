//! `cargo xtask env-docs` (#318): every `OPENLENS_*` variable the server
//! reads must be documented in `docs/deploy/README.md`.
//!
//! It scans the server side (`crates/` and `apps/openlens`, tests
//! excluded) for full names in string literals. Prefixes such as
//! `"OPENLENS_LLM_"` end with `_` and are not names. The CLI
//! (`apps/openlens-cli`) reads client-side variables, documented with the
//! CLI, and is not scanned.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DOC: &str = "docs/deploy/README.md";
const ROOTS: &[&str] = &["crates", "apps/openlens"];

/// The full `OPENLENS_*` names in string literals of `src`.
fn names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("\"OPENLENS_") {
        let after = &rest[i + 1..];
        let end = after
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..end];
        if after[end..].starts_with('"') && !name.ends_with('_') {
            out.push(name.to_owned());
        }
        rest = &after[end..];
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            if name != "tests" && name != "target" {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Names read by the code but absent from `doc`, with one file each.
fn undocumented(files: &[(PathBuf, String)], doc: &str) -> BTreeMap<String, PathBuf> {
    let mut missing = BTreeMap::new();
    for (path, src) in files {
        for n in names(src) {
            if !doc.contains(&n) {
                missing.entry(n).or_insert_with(|| path.clone());
            }
        }
    }
    missing
}

pub(crate) fn check() -> Result<(), String> {
    let doc = std::fs::read_to_string(DOC).map_err(|e| format!("cannot read {DOC}: {e}"))?;
    let mut paths = Vec::new();
    for r in ROOTS {
        rust_files(Path::new(r), &mut paths);
    }
    let files: Vec<(PathBuf, String)> = paths
        .into_iter()
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|s| (p, s)))
        .collect();
    let missing = undocumented(&files, &doc);
    if missing.is_empty() {
        eprintln!("xtask: env-docs ok (every OPENLENS_* variable is in {DOC})");
        return Ok(());
    }
    let list: Vec<String> = missing
        .iter()
        .map(|(n, p)| format!("{n} (read in {})", p.display()))
        .collect();
    Err(format!(
        "document these variables in {DOC}: {}",
        list.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn names_skip_prefixes_and_keep_full_names() {
        let src =
            r#"env::var("OPENLENS_SMTP_URL"); let p = "OPENLENS_LLM_"; get("OPENLENS_S3_BUCKET")"#;
        assert_eq!(
            super::names(src),
            ["OPENLENS_SMTP_URL", "OPENLENS_S3_BUCKET"]
        );
    }

    #[test]
    fn an_undocumented_variable_is_reported() {
        let files = vec![(
            PathBuf::from("a.rs"),
            r#"std::env::var("OPENLENS_FOO"); std::env::var("OPENLENS_ADDR")"#.to_owned(),
        )];
        let missing = super::undocumented(&files, "| `OPENLENS_ADDR` |");
        assert_eq!(missing.keys().collect::<Vec<_>>(), ["OPENLENS_FOO"]);
    }
}
