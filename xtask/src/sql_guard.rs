//! `cargo xtask sql-guard` (part of `ci`, #310): SQL values are bound or
//! quoted by `openlens-sqlgen` only. Doubling quotes by hand anywhere else
//! fails the build, so a new query cannot splice a value in its own way.

use std::path::{Path, PathBuf};

/// The hand-made escape this check looks for.
const NEEDLE: &str = r#"replace('\'', "''")"#;

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if e.file_name() != "target" {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Files outside `crates/sqlgen` (and this check) that escape quotes.
fn offenders(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for d in ["crates", "apps"] {
        rust_files(&root.join(d), &mut files);
    }
    files.sort();
    let allowed = root.join("crates").join("sqlgen");
    files
        .into_iter()
        .filter(|f| !f.starts_with(&allowed))
        .filter(|f| std::fs::read_to_string(f).is_ok_and(|s| s.contains(NEEDLE)))
        .collect()
}

/// Fail when a quote is escaped by hand outside `openlens-sqlgen`.
pub fn check() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no workspace root")?
        .to_path_buf();
    let bad = offenders(&root);
    if bad.is_empty() {
        eprintln!("xtask: SQL values go through openlens-sqlgen only");
        return Ok(());
    }
    let list: Vec<String> = bad
        .iter()
        .map(|f| format!("  {}", f.strip_prefix(&root).unwrap_or(f).display()))
        .collect();
    Err(format!(
        "quotes escaped by hand; use openlens_sqlgen (Params, lit or quote) instead (#310):\n{}",
        list.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_made_escape_outside_sqlgen_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["crates/sqlgen/src", "crates/engine/src"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let code = format!("fn f(v: &str) -> String {{ v.{NEEDLE} }}\n");
        std::fs::write(root.join("crates/sqlgen/src/lib.rs"), &code).unwrap();
        std::fs::write(root.join("crates/engine/src/ok.rs"), "fn g() {}\n").unwrap();
        assert!(offenders(root).is_empty());
        std::fs::write(root.join("crates/engine/src/bad.rs"), &code).unwrap();
        assert_eq!(offenders(root), [root.join("crates/engine/src/bad.rs")]);
    }
}
