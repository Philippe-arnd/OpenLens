//! `features lint`: the map is valid, and it matches the app's code.

use std::path::Path;

use serde_json::{Value, json};

const SKIP: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    "vendor",
    "coverage",
];
const EXT: &[&str] = &[
    "html", "htm", "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte", "astro",
    "erb", "haml", "slim", "py", "rb", "php", "ex", "exs", "heex", "eex", "jinja", "j2", "twig",
    "blade", "hbs", "mustache", "rs", "go", "cs", "cshtml",
];
const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Collect the text of the app's source files (bounded).
fn scan(root: &Path) -> String {
    let mut out = String::new();
    let mut stack = vec![root.to_path_buf()];
    let mut files = 0;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(p);
                }
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| EXT.contains(&x))
                && e.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES)
            {
                files += 1;
                if files > MAX_FILES {
                    return out;
                }
                if let Ok(t) = std::fs::read_to_string(&p) {
                    out.push_str(&t);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Lint report as JSON.
pub fn lint(map_text: &str, src: &Path) -> Value {
    let parsed = openlens_featuremap::parse(map_text);
    let mut issues: Vec<Value> = parsed.issues.iter().map(|i| json!(i)).collect();
    if let Some(m) = &parsed.map {
        let code = scan(src);
        for (i, f) in m.features.iter().enumerate() {
            let has_attr = code.contains(&format!("data-feature=\"{}\"", f.key))
                || code.contains(&format!("data-feature='{}'", f.key))
                || code.contains(&format!("feature(\"{}\"", f.key))
                || code.contains(&format!("feature('{}'", f.key));
            let route_found = f.entry.routes.iter().any(|r| {
                let stem = r
                    .split(':')
                    .next()
                    .unwrap_or(r)
                    .trim_end_matches('*')
                    .trim_end_matches('/');
                !stem.is_empty() && code.contains(stem)
            });
            if !has_attr && !route_found {
                issues.push(json!({ "level": "warning", "path": format!("features[{i}]"),
                    "message": format!("nothing in the code points to `{}`: add data-feature=\"{}\" to its main control or check its routes", f.key, f.key) }));
            }
            if let Some(s) = &f.success
                && !s.starts_with('$')
                && !code.contains(s.as_str())
            {
                issues.push(json!({ "level": "warning", "path": format!("features[{i}].success"),
                    "message": format!("`{s}` is never sent by the code: add openlens.track(\"{s}\") where the user gets value") }));
            }
        }
    }
    let errors = issues.iter().filter(|i| i["level"] == "error").count();
    json!({ "valid": parsed.map.is_some(), "errors": errors, "warnings": issues.len() - errors, "issues": issues })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_references_silence_warnings_and_missing_ones_raise_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("app.tsx"),
            r#"<button data-feature="export">x</button>"#,
        )
        .unwrap();
        // ES module servers (.mjs) are app code too.
        std::fs::write(
            dir.path().join("server.mjs"),
            r#"openlens.track("exported")"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        std::fs::write(
            dir.path().join("node_modules/x.js"),
            r#"data-feature="ghost""#,
        )
        .unwrap();
        let map = "roles:\n  - key: a\n    match: [a]\nfeatures:\n  - key: export\n    name: Export\n    success: exported\n    entry: { selectors: ['[data-feature=\"export\"]'] }\n  - key: ghost\n    name: Ghost\n    success: haunted\n    entry: { selectors: ['[data-feature=\"ghost\"]'] }\n";
        let r = lint(map, dir.path());
        let paths: Vec<_> = r["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["path"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            !paths.iter().any(|p| p.starts_with("features[0]")),
            "export is wired: {paths:?}"
        );
        assert!(
            paths.contains(&"features[1]".to_owned()),
            "node_modules is not the app's code"
        );
        assert!(paths.contains(&"features[1].success".to_owned()));
    }
}
