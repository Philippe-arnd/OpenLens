//! Repository automation.
//!
//! - `cargo xtask ci`: everything CI runs (fmt, clippy, tests, layers, deny).
//! - `cargo xtask layers`: check crate layering against `src/layers.rs`.
//! - `cargo xtask demo <data-dir>`: write a realistic synthetic app (300
//!   users, planted patterns) into the `default` project of a data dir, to
//!   look at the board: `OPENLENS_DATA_DIR=<data-dir> cargo run -p openlens`.

mod layers;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "layers" => check_layers(),
        "demo" => demo(std::env::args().nth(2)),
        _ => Err("usage: cargo xtask <ci|layers|demo DIR>".to_owned()),
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
    let (mut w, _) = openlens_storage::Writer::open(&dir, openlens_storage::FlushPolicy::default())
        .map_err(|e| e.to_string())?;
    let events = openlens_testkit::planted_dataset("default");
    w.append(&events).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    eprintln!(
        "xtask: wrote {} demo events to {dir} (project default)",
        events.len()
    );
    Ok(())
}
