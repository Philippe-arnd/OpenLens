//! `cargo xtask file-length` (part of `ci`, #307): no Rust file of the
//! workspace above [`MAX_LINES`], tests included, so no file grows back
//! into a monolith. Split a file into modules when it gets there.

use std::path::{Path, PathBuf};

/// The longest a `.rs` file may be.
pub const MAX_LINES: usize = 1000;

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

/// Files over the limit, with their length, under the given roots.
fn too_long(roots: &[PathBuf]) -> Vec<(PathBuf, usize)> {
    let mut files = Vec::new();
    for r in roots {
        rust_files(r, &mut files);
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            let n = std::fs::read_to_string(&f).ok()?.lines().count();
            (n > MAX_LINES).then_some((f, n))
        })
        .collect()
}

/// Fail when a workspace Rust file is longer than [`MAX_LINES`].
pub fn check() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no workspace root")?
        .to_path_buf();
    let roots: Vec<PathBuf> = ["crates", "apps", "xtask", "fuzz"]
        .iter()
        .map(|d| root.join(d))
        .collect();
    let long = too_long(&roots);
    if long.is_empty() {
        eprintln!("xtask: every Rust file has at most {MAX_LINES} lines");
        return Ok(());
    }
    let list: Vec<String> = long
        .iter()
        .map(|(f, n)| {
            let shown = f.strip_prefix(&root).unwrap_or(f);
            format!("  {} ({n} lines)", shown.display())
        })
        .collect();
    Err(format!(
        "files over {MAX_LINES} lines; split them into modules (#307):\n{}",
        list.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_over_the_limit_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("ok.rs"), "x\n".repeat(MAX_LINES)).unwrap();
        std::fs::write(sub.join("long.rs"), "x\n".repeat(MAX_LINES + 1)).unwrap();
        std::fs::write(sub.join("notes.md"), "x\n".repeat(MAX_LINES * 2)).unwrap();
        let long = too_long(&[dir.path().to_path_buf()]);
        assert_eq!(long, vec![(sub.join("long.rs"), MAX_LINES + 1)]);
    }
}
