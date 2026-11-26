//! Uploading the source maps of a build (moved out of main.rs).

use std::path::Path;

use crate::{Body, Client, out};

/// Upload every `*.map` file under `dir` for `release`.
pub(crate) fn sourcemaps(
    c: &Client,
    project: &str,
    release: &str,
    dir: &Path,
) -> Result<bool, String> {
    let mut maps = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries =
            std::fs::read_dir(&d).map_err(|e| format!("cannot read {}: {e}", d.display()))?;
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "map") {
                maps.push(p);
            }
        }
    }
    let rel: String = release
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || "-_.@+".contains(ch) {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for p in &maps {
        let Some(bundle) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let text =
            std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        let (status, v) = c.call(
            "PUT",
            &format!("/api/projects/{project}/sourcemaps/{rel}/{bundle}"),
            Some(Body::Text(text)),
        )?;
        if status == 200 {
            ok.push(bundle.to_owned());
        } else {
            failed
                .push(serde_json::json!({ "file": p.display().to_string(), "error": v["error"] }));
        }
    }
    out(&serde_json::json!({ "release": rel, "uploaded": ok, "failed": failed }));
    Ok(failed.is_empty() && !maps.is_empty())
}
