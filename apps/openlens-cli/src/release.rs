//! `openlens-cli release` (#521, part 4 of #237, D-066): record a release
//! from the deploy step with its commit range, upload its source maps, and
//! wait for its verdict when asked. Also `openlens-cli sourcemaps`, the
//! upload on its own.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{Body, Client, api_error, out, query_value};

/// How long `--wait` polls for the verdict.
const WAIT: Duration = Duration::from_secs(30 * 60);
/// Time between two polls.
const POLL: Duration = Duration::from_secs(30);

/// Upload every `*.map` file under `dir` for `release`.
pub(crate) fn sourcemaps(
    c: &Client,
    project: &str,
    release: &str,
    dir: &Path,
) -> Result<bool, String> {
    let (v, ok) = upload(c, project, release, dir)?;
    out(&v);
    Ok(ok)
}

/// Upload every `*.map` file under `dir` for `release`: what was uploaded
/// and what failed, and whether at least one was and none failed.
fn upload(c: &Client, project: &str, release: &str, dir: &Path) -> Result<(Value, bool), String> {
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
            failed.push(json!({ "file": p.display().to_string(), "error": v["error"] }));
        }
    }
    let good = failed.is_empty() && !maps.is_empty();
    Ok((
        json!({ "release": rel, "uploaded": ok, "failed": failed }),
        good,
    ))
}

/// What `openlens-cli release` was given.
pub(crate) struct Args<'a> {
    pub(crate) project: &'a str,
    pub(crate) name: &'a str,
    pub(crate) commit: Option<&'a str>,
    pub(crate) from: Option<&'a str>,
    pub(crate) env: &'a str,
    pub(crate) sourcemaps: Option<&'a Path>,
    pub(crate) wait: bool,
}

/// Record the release (`releases.record`), upload its source maps, and
/// with `wait` poll the releases list until its verdict is there. Prints
/// one JSON document; `Ok(false)` when an upload failed, the verdict is
/// regressed, or it did not come in time.
pub(crate) fn release(c: &Client, a: &Args<'_>) -> Result<bool, String> {
    let (status, recorded) = c.call(
        "POST",
        &format!("/api/projects/{}/releases", a.project),
        Some(Body::Json(json!({
            "release": a.name,
            "commit": a.commit,
            "from": a.from,
            "environment": a.env,
        }))),
    )?;
    if status != 200 {
        return Err(format!(
            "the release was not recorded: {}",
            api_error(status, &recorded)
        ));
    }
    let mut report = json!({ "recorded": recorded });
    let mut ok = true;
    if let Some(dir) = a.sourcemaps {
        let (v, good) = upload(c, a.project, a.name, dir)?;
        report["sourcemaps"] = v;
        ok &= good;
    }
    if a.wait {
        // The name and environment as the server kept them.
        let name = recorded["release"].as_str().unwrap_or(a.name);
        let env = recorded["environment"].as_str().unwrap_or(a.env);
        match wait_for_verdict(c, a.project, name, env)? {
            Some(v) => {
                ok &= !regressed(&v);
                report["verdict"] = v;
            }
            None => {
                ok = false;
                report["verdict"] = Value::Null;
                report["note"] = json!(format!(
                    "no verdict after {} minutes: it needs enough users of the release, or 24 hours. \
                     Read it later with MCP `releases` (or GET /api/projects/{}/releases).",
                    WAIT.as_secs() / 60,
                    a.project
                ));
            }
        }
    }
    out(&report);
    Ok(ok)
}

/// Poll the project's releases until `name` in `env` has a verdict, for
/// at most [`WAIT`].
fn wait_for_verdict(
    c: &Client,
    project: &str,
    name: &str,
    env: &str,
) -> Result<Option<Value>, String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let (status, v) = c.call(
            "GET",
            &format!("/api/projects/{project}/releases?env={}", query_value(env)),
            None,
        )?;
        if status != 200 {
            return Err(format!(
                "cannot read the releases: {}",
                api_error(status, &v)
            ));
        }
        if let Some(verdict) = verdict_of(&v, name, env) {
            return Ok(Some(verdict));
        }
        if Instant::now() + POLL > deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

/// The verdict of `name` in `env` in a releases list, once it has one.
fn verdict_of(list: &Value, name: &str, env: &str) -> Option<Value> {
    list["releases"]
        .as_array()?
        .iter()
        .find(|r| r["release"] == name && r["environment"] == env)
        .map(|r| r["verdict"].clone())
        .filter(|v| !v.is_null())
}

/// Whether a verdict says the release regressed: the word itself, or an
/// object whose `verdict`, `status` or `state` is.
fn regressed(v: &Value) -> bool {
    let word = |x: &Value| x.as_str() == Some("regressed");
    word(v) || ["verdict", "status", "state"].iter().any(|k| word(&v[*k]))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::Cli;
    use crate::commands::Command;

    #[test]
    fn release_takes_its_range_source_maps_and_wait() {
        let cli = Cli::try_parse_from([
            "openlens-cli",
            "release",
            "v1.4.0",
            "--commit",
            "0123456789abcdef",
            "--from",
            "fedcba9876543210",
            "--env",
            "staging",
            "--sourcemaps",
            "dist",
            "--wait",
        ])
        .unwrap();
        let Command::Release {
            name,
            project,
            commit,
            from,
            env,
            sourcemaps,
            wait,
        } = cli.command
        else {
            panic!("not the release command");
        };
        assert_eq!(name, "v1.4.0");
        assert_eq!(project, "default");
        assert_eq!(commit.as_deref(), Some("0123456789abcdef"));
        assert_eq!(from.as_deref(), Some("fedcba9876543210"));
        assert_eq!(env, "staging");
        assert_eq!(sourcemaps, Some(std::path::PathBuf::from("dist")));
        assert!(wait);

        // Only the name is required; the range then starts at the
        // previous release, and nothing waits.
        let cli = Cli::try_parse_from(["openlens-cli", "release", "v2"]).unwrap();
        let Command::Release {
            env, from, wait, ..
        } = cli.command
        else {
            panic!("not the release command");
        };
        assert_eq!((env.as_str(), from, wait), ("production", None, false));
        assert!(Cli::try_parse_from(["openlens-cli", "release"]).is_err());
    }

    #[test]
    fn the_wait_stops_on_this_release_s_verdict_only() {
        let list = json!({"releases": [
            {"release": "v2", "environment": "production", "verdict": null},
            {"release": "v2", "environment": "staging", "verdict": "healthy"},
            {"release": "v1", "environment": "production", "verdict": {"verdict": "regressed"}},
            {"release": "v3", "environment": "production"},
        ]});
        assert_eq!(verdict_of(&list, "v2", "production"), None, "not yet");
        assert_eq!(
            verdict_of(&list, "v3", "production"),
            None,
            "no verdict field"
        );
        assert_eq!(
            verdict_of(&list, "v9", "production"),
            None,
            "not listed yet"
        );
        let staged = verdict_of(&list, "v2", "staging").unwrap();
        assert!(!regressed(&staged));
        let old = verdict_of(&list, "v1", "production").unwrap();
        assert!(regressed(&old));
    }
}
