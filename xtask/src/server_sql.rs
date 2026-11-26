//! `cargo xtask server-sql` (part of `ci`, #509, D-058): the server holds
//! no SQL. Every query lives in the engine (rule 10), so a Rust string
//! literal in `crates/server/src` that reads like a query (`select … from`,
//! `insert into`, `update <name> set`, `delete from`, `with <name> as (`,
//! in any case) fails the build. Comments and test code (`#[cfg(test)]`)
//! are not checked: a test may send SQL to an endpoint.

use std::path::Path;

/// Server files allowed to hold SQL. Empty since #507/#508 moved Ask and
/// explore to the engine; an entry with no SQL left fails the check, so the
/// list cannot go stale.
const ALLOWED: &[&str] = &[];

/// Words a sentence puts after "select" ("select a project from the
/// list"); a query never does.
const PROSE_AFTER_SELECT: &[&str] = &[
    "a", "an", "the", "one", "your", "which", "what", "all", "any", "some", "each", "every",
    "this", "that", "it", "them", "from",
];

/// Lowercase words and single punctuation characters of `s`.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        if !ch.is_whitespace() {
            out.push(ch.to_string());
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// True when `s` reads like a query rather than prose.
pub(crate) fn looks_like_sql(s: &str) -> bool {
    let t = tokens(s);
    let word = |k: usize| t.get(k).map_or("", String::as_str);
    let name = |w: &str| w.starts_with(|ch: char| ch.is_alphabetic() || ch == '_');
    (0..t.len()).any(|k| match word(k) {
        "insert" => word(k + 1) == "into",
        "delete" => word(k + 1) == "from",
        "update" => name(word(k + 1)) && word(k + 2) == "set",
        "select" => {
            !PROSE_AFTER_SELECT.contains(&word(k + 1)) && t.iter().skip(k + 1).any(|w| w == "from")
        }
        "with" => name(word(k + 1)) && word(k + 2) == "as" && word(k + 3) == "(",
        _ => false,
    })
}

/// The string literals of Rust source `src` outside comments and test
/// code, with the line each starts on (escapes kept as written). Test
/// code is the item after `#[cfg(test)]`: its `{ … }` block, or nothing
/// when a `;` ends it first.
pub(crate) fn literals(src: &str) -> Vec<(usize, String)> {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    let starts = |i: usize, p: &str| p.chars().enumerate().all(|(k, x)| at(i + k) == x);
    let ident = |x: char| x.is_alphanumeric() || x == '_';
    let mut out = Vec::new();
    let (mut i, mut line, mut depth) = (0, 1, 0usize);
    // `#[cfg(test)]` seen, its item not entered yet; the depth of the
    // test block being skipped.
    let (mut marked, mut skip) = (false, None::<usize>);
    let mut keep = |line: usize, text: String, skip: Option<usize>| {
        if skip.is_none() {
            out.push((line, text));
        }
    };
    while i < c.len() {
        let x = at(i);
        if x == '\n' {
            line += 1;
            i += 1;
        } else if starts(i, "//") {
            while i < c.len() && at(i) != '\n' {
                i += 1;
            }
        } else if starts(i, "/*") {
            let mut nest = 0usize;
            while i < c.len() {
                if starts(i, "/*") {
                    nest += 1;
                    i += 2;
                } else if starts(i, "*/") {
                    nest = nest.saturating_sub(1);
                    i += 2;
                } else {
                    line += usize::from(at(i) == '\n');
                    i += 1;
                }
                if nest == 0 {
                    break;
                }
            }
        } else if starts(i, "#[cfg(test)]") {
            marked = true;
            i += "#[cfg(test)]".len();
        } else if ident(x) {
            let begin = i;
            while ident(at(i)) {
                i += 1;
            }
            let prefix: String = c.get(begin..i).unwrap_or_default().iter().collect();
            let mut j = i;
            while at(j) == '#' {
                j += 1;
            }
            if (prefix == "r" || prefix == "br") && at(j) == '"' {
                // A raw string ends at a quote followed by as many `#`.
                let hashes = j - i;
                let start = j + 1;
                let mut k = start;
                while k < c.len() && !(at(k) == '"' && (1..=hashes).all(|h| at(k + h) == '#')) {
                    k += 1;
                }
                let text: String = c.get(start..k).unwrap_or_default().iter().collect();
                let lines = text.matches('\n').count();
                keep(line, text, skip);
                line += lines;
                i = k + 1 + hashes;
            }
        } else if x == '\'' {
            // A char literal ('x', '\n', '\'', '"'), or a lifetime.
            if at(i + 1) == '\\' {
                let mut k = i + 3;
                while k < c.len() && at(k) != '\'' {
                    k += 1;
                }
                i = k + 1;
            } else if at(i + 2) == '\'' {
                i += 3;
            } else {
                i += 1;
            }
        } else if x == '"' {
            let mut k = i + 1;
            let mut text = String::new();
            while k < c.len() && at(k) != '"' {
                if at(k) == '\\' {
                    text.push('\\');
                    k += 1;
                }
                text.push(at(k));
                k += 1;
            }
            let lines = text.matches('\n').count();
            keep(line, text, skip);
            line += lines;
            i = k + 1;
        } else {
            match x {
                '{' => {
                    if marked {
                        skip.get_or_insert(depth);
                        marked = false;
                    }
                    depth += 1;
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    if skip == Some(depth) {
                        skip = None;
                    }
                }
                ';' if skip.is_none() => marked = false,
                _ => {}
            }
            i += 1;
        }
    }
    out
}

/// `file:line` of each query-like literal under `root`'s server sources,
/// skipping the `allowed` files, plus each allowed file that has none.
fn offenders(root: &Path, allowed: &[&str]) -> Vec<String> {
    let mut files = Vec::new();
    crate::sql_guard::rust_files(&root.join("crates").join("server").join("src"), &mut files);
    files.sort();
    let mut bad = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(src) = std::fs::read_to_string(&f) else {
            bad.push(format!("  {rel}: unreadable"));
            continue;
        };
        let hits: Vec<usize> = literals(&src)
            .into_iter()
            .filter(|(_, text)| looks_like_sql(text))
            .map(|(line, _)| line)
            .collect();
        match (allowed.contains(&rel.as_str()), hits.is_empty()) {
            (true, true) => bad.push(format!(
                "  {rel}: no SQL left, remove it from ALLOWED in xtask/src/server_sql.rs"
            )),
            (false, false) => bad.extend(hits.iter().map(|l| format!("  {rel}:{l}"))),
            _ => {}
        }
    }
    bad
}

/// Fail when a server source file holds a query.
pub fn check() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no workspace root")?;
    let bad = offenders(root, ALLOWED);
    if bad.is_empty() {
        eprintln!("xtask: no SQL in crates/server/src");
        return Ok(());
    }
    Err(format!(
        "SQL in the server; move the query into crates/engine and call it (rule 10, D-058):\n{}",
        bad.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_found_in_any_case_and_prose_is_not() {
        for q in [
            "SELECT feature, count(*) AS n FROM events",
            "select distinct_id from events where date >= '{since}'",
            "(SELECT *, distinct_id AS user_key FROM {src}) AS events",
            "INSERT INTO goals (id) VALUES (?1)",
            "update goals set target = ?1",
            "Delete From events",
            "WITH t AS (SELECT 1) SELECT * FROM t",
        ] {
            assert!(looks_like_sql(q), "{q}");
        }
        for prose in [
            "select a project",
            "Select the release from the list",
            "Select from the menu",
            "Update your settings",
            "Delete the project, then insert the key",
            "Events with a role",
            "with the team as (optional) reviewers",
            "project.delete",
            "/p/{project}/goals/{gid}/delete",
        ] {
            assert!(!looks_like_sql(prose), "{prose}");
        }
    }

    #[test]
    fn comments_char_literals_and_test_code_are_not_literals() {
        let src = r##"
// let q = "SELECT a FROM b";
/* "DELETE FROM b" */
fn f<'a>(x: &'a str) -> char { let _ = '"'; let _ = '\''; '{' }
const Q: &str = r#"select x from "y""#;
#[cfg(test)]
mod tests { fn g() { let _ = "SELECT a FROM b"; { "x"; } } }
#[cfg(test)]
use std::fmt;
const R: &str = "delete from t";
"##;
        let found: Vec<_> = literals(src)
            .into_iter()
            .filter(|(_, t)| looks_like_sql(t))
            .map(|(line, _)| line)
            .collect();
        assert_eq!(found, [5, 10]);
    }

    #[test]
    fn a_query_in_the_server_fails_unless_allowed_and_a_stale_allowance_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let src = root.join("crates/server/src");
        std::fs::create_dir_all(src.join("web")).unwrap();
        std::fs::write(
            src.join("ok.rs"),
            "fn f() -> &'static str { \"Select a day\" }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("web/bad.rs"),
            "fn f() {}\nconst Q: &str = \"SELECT a FROM b\";\n",
        )
        .unwrap();
        assert_eq!(offenders(root, &[]), ["  crates/server/src/web/bad.rs:2"]);
        assert!(offenders(root, &["crates/server/src/web/bad.rs"]).is_empty());
        assert_eq!(
            offenders(
                root,
                &["crates/server/src/web/bad.rs", "crates/server/src/ok.rs"]
            )
            .len(),
            1
        );
    }
}
