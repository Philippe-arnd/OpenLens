//! Repository automation.
//!
//! - `cargo xtask ci`: everything CI runs (fmt, clippy, tests, layers, deny).
//! - `cargo xtask layers`: check crate layering against `src/layers.rs`.
//! - `cargo xtask demo <data-dir>`: write a realistic synthetic app (300
//!   users, planted patterns) into the `default` project of a data dir, to
//!   look at the board: `OPENLENS_DATA_DIR=<data-dir> cargo run -p openlens`.
//! - `cargo xtask demo-site <dir> [--bin PATH]`: the same data set as a
//!   static, read-only site crawled from the real server (#304).
//! - `cargo xtask bench <url> <key>`: ingestion load test (docs/benchmarks.md).
//! - `cargo xtask bench-gen <data-dir> [--events N] [--users N] [--profile realistic]`: a large
//!   synthetic app (63 M events over 90 days by default) for scale runs (#321).
//! - `cargo xtask crash [--secs N] [--kills N] [--seed N]`: SIGKILL the real
//!   server while it ingests, and check no acknowledged event is lost or
//!   stored twice (#331).
//! - `cargo xtask env-docs`: every `OPENLENS_*` variable the server reads is
//!   in docs/deploy/README.md (#318; part of `ci`).
//! - `cargo xtask sql-guard`: quotes are escaped by openlens-sqlgen only
//!   (#310; part of `ci`).
//! - `cargo xtask file-length`: no Rust file above 1,000 lines (#307; part
//!   of `ci`).

mod bench;
mod bench_gen;
mod crash;
mod demo_site;
mod env_docs;
mod file_length;
mod layers;
mod sql_guard;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "layers" => check_layers(),
        "demo" => demo(std::env::args().nth(2)),
        "demo-site" => demo_site::demo_site(&std::env::args().skip(2).collect::<Vec<_>>()),
        "bench" => bench::bench(&std::env::args().skip(2).collect::<Vec<_>>()),
        "bench-gen" => bench_gen::bench_gen(&std::env::args().skip(2).collect::<Vec<_>>()),
        "crash" => crash::crash(&std::env::args().skip(2).collect::<Vec<_>>()),
        "env-docs" => env_docs::check(),
        "file-length" => file_length::check(),
        "sql-guard" => sql_guard::check(),
        _ => Err(
            "usage: cargo xtask <ci|layers|env-docs|file-length|sql-guard|demo DIR|demo-site DIR|bench URL KEY|bench-gen DIR|crash [--secs N] [--kills N] [--seed N]>"
                .to_owned(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("xtask: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    eprintln!("xtask: running {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} failed", args.join(" ")))
    }
}

fn ci() -> Result<(), String> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run("cargo", &["test", "--workspace", "--all-targets"])?;
    check_layers()?;
    env_docs::check()?;
    file_length::check()?;
    sql_guard::check()?;
    // cargo-deny is optional locally; CI installs it. Say so instead of
    // silently skipping.
    if Command::new("cargo")
        .args(["deny", "--version"])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        run("cargo", &["deny", "check", "licenses", "bans", "sources"])?;
    } else {
        eprintln!("xtask: cargo-deny not installed, licence check NOT run");
    }
    Ok(())
}

fn check_layers() -> Result<(), String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err("cargo metadata failed".to_owned());
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad cargo metadata: {e}"))?;
    let packages = meta["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;

    let mut errors = Vec::new();
    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or_default();
        let Ok(layer) = layers::layer_of(name) else {
            errors.push(format!("{name} is not registered in xtask/src/layers.rs"));
            continue;
        };
        for dep in pkg["dependencies"].as_array().into_iter().flatten() {
            let dep_name = dep["name"].as_str().unwrap_or_default();
            let Ok(dep_layer) = layers::layer_of(dep_name) else {
                continue; // external crate
            };
            let is_dev = dep["kind"].as_str() == Some("dev");
            match (layer, dep_layer) {
                (_, None) if is_dev => {}
                // Tooling (xtask, testkit) may use other tooling.
                (None, None) => {}
                (_, None) => errors.push(format!(
                    "{name} depends on test helper {dep_name} outside dev-dependencies"
                )),
                (Some(l), Some(d)) if d >= l => errors.push(format!(
                    "{name} (L{l}) depends on {dep_name} (L{d}): dependencies must be lower"
                )),
                _ => {}
            }
        }
    }
    if errors.is_empty() {
        eprintln!("xtask: layers ok ({} crates)", packages.len());
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn demo(dir: Option<String>) -> Result<(), String> {
    let dir = dir.ok_or("usage: cargo xtask demo <data-dir>")?;
    write_demo(std::path::Path::new(&dir))
}

/// The demo data set (300 users, planted patterns) in the `default` project.
fn write_demo(dir: &std::path::Path) -> Result<(), String> {
    let (mut w, _) = openlens_storage::Writer::open(dir, openlens_storage::FlushPolicy::default())
        .map_err(|e| e.to_string())?;
    let events = openlens_testkit::planted_dataset("default");
    w.append(&events).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    let meta = openlens_meta::Meta::open(&dir.join("meta.db")).map_err(|e| e.to_string())?;
    meta.ensure_default_project(None)
        .map_err(|e| e.to_string())?;
    eprintln!(
        "xtask: wrote {} demo events to {} (project default)",
        events.len(),
        dir.display()
    );
    Ok(())
}
