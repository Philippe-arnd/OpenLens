//! `cargo xtask bench-gen <data-dir> [--events N] [--users N]` (#321):
//! write a large synthetic app (63 M events over 90 days by default) into
//! a data dir, for scale benchmarks: start the server on it and measure
//! the board. Writes through the store's own writer in chunks, then
//! compacts each day, so the files look like a server's after 90 days.
//! Deterministic. A small run (`--events 1000000`) suits CI.
//!
//! `--profile realistic` (ARCH-10, #443) writes the realistic scenario
//! instead: about 160 events per user, Pareto-skewed activity, real
//! sessions, `$identify`, six raw roles. `--users` is then ignored (a run
//! needs `events / 160` users). `--map-out FILE` writes the matching
//! 24-feature, 3-role feature map, and stores it as the project's map
//! (version 1): pushing the same file later keeps that version (D-046).
//!
//! Then the closed days are rolled up (#322, D-025, D-038, D-041) with the
//! server's own functions and that map, so the board reads what a server
//! running for weeks would (D-040), not only events. `--no-rollups` skips
//! it; the server's background pass then rolls 7 days per 10 minutes.

use std::time::Instant;

const CHUNK: u64 = 250_000;

pub(crate) fn bench_gen(args: &[String]) -> Result<(), String> {
    let usage = "usage: cargo xtask bench-gen <data-dir> [--events N] [--users N] [--profile realistic] [--map-out FILE] [--no-rollups]";
    let dir = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or(usage)?
        .clone();
    let mut events: u64 = 63_000_000;
    let mut users: u64 = 200_000;
    let mut realistic = false;
    let mut map_out: Option<String> = None;
    let mut rollups = true;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--no-rollups" {
            rollups = false;
            continue;
        }
        let v = it.next().ok_or(usage)?;
        let number = || v.replace('_', "").parse::<u64>().map_err(|_| usage);
        match a.as_str() {
            "--events" => events = number()?,
            "--users" => users = number()?,
            "--profile" if v == "realistic" => realistic = true,
            "--map-out" => map_out = Some(v.clone()),
            _ => return Err(usage.into()),
        }
    }
    let map_yaml = map_out
        .as_ref()
        .map(|_| openlens_testkit::realistic_feature_map());
    if let (Some(path), Some(yaml)) = (&map_out, &map_yaml) {
        std::fs::write(path, yaml).map_err(|e| e.to_string())?;
    }
    let (mut w, _) = openlens_storage::Writer::open(&dir, openlens_storage::FlushPolicy::default())
        .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let started = Instant::now();
    let mut done = 0;
    let mut next_user = 0;
    while done < events {
        let chunk = if realistic {
            realistic_chunk(&mut next_user, events - done, now)
        } else {
            openlens_testkit::scale_chunk("default", events, users, done, CHUNK, now)
        };
        if chunk.is_empty() {
            return Err("no events generated".into());
        }
        done += chunk.len() as u64;
        w.append(&chunk).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())?;
        if done % (CHUNK * 20) == 0 || done == events {
            eprintln!(
                "xtask: {done} / {events} events written ({:.0} s)",
                started.elapsed().as_secs_f64()
            );
        }
    }
    let days = openlens_storage::compact(std::path::Path::new(&dir), "default", now)
        .map_err(|e| e.to_string())?;
    let meta = openlens_meta::Meta::open(&std::path::Path::new(&dir).join("meta.db"))
        .map_err(|e| e.to_string())?;
    meta.ensure_default_project(None)
        .map_err(|e| e.to_string())?;
    let map = match &map_yaml {
        Some(yaml) => {
            let version = meta
                .save_feature_map("default", yaml)
                .map_err(|e| e.to_string())?;
            let parsed = openlens_featuremap::parse(yaml)
                .map
                .ok_or("the feature map does not parse")?;
            Some((parsed, version))
        }
        None => None,
    };
    let rolled = Instant::now();
    let days_rolled = if rollups {
        roll_up(std::path::Path::new(&dir), "default", map.as_ref(), now)?
    } else {
        0
    };
    let rollup_seconds = rolled.elapsed().as_secs_f64();
    if realistic {
        users = next_user;
    }
    println!(
        "{}",
        serde_json::json!({
            "events": events,
            "users": users,
            "profile": if realistic { "realistic" } else { "uniform" },
            "days_compacted": days,
            "days_rolled": days_rolled,
            "rollup_seconds": (rollup_seconds * 10.0).round() / 10.0,
            "seconds": started.elapsed().as_secs(),
            "dir": dir,
        })
    );
    Ok(())
}

/// Roll up every closed day of `project` lacking a rollup, as the
/// server's background pass does (same functions, same feature lookup and
/// map version, same memory budget), but all of them at once. Returns the
/// number of days written.
fn roll_up(
    root: &std::path::Path,
    project: &str,
    map: Option<&(openlens_featuremap::FeatureMap, i64)>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<usize, String> {
    use openlens_storage::rollup;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    // The server's memory budget; a longer timeout, as nothing else waits
    // on this process and a debug build is slow (one failed day fails all).
    let limits = openlens_query::Limits {
        timeout: std::time::Duration::from_secs(600),
        ..openlens_query::Limits::from_host()
    };
    let query = openlens_query::QueryEngine::new(root, limits);
    let features = openlens_engine::rollups::day_features(map.map(|(m, v)| (m, *v)));
    let days = rollup::missing(root, project, now).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut written = 0;
    for (i, day) in days.iter().enumerate() {
        let generation = rollup::generation();
        let label = day.format("%Y-%m-%d").to_string();
        let lacking = rollup::lacking(root, project, *day);
        let batches = rt
            .block_on(query.rollup_day(project, &label, Some(&lacking), &features))
            .map_err(|e| format!("rollup of {label}: {e}"))?;
        if rollup::write(root, project, *day, generation, &batches).map_err(|e| e.to_string())? {
            written += 1;
        }
        if (i + 1) % 10 == 0 || i + 1 == days.len() {
            eprintln!(
                "xtask: {} / {} days rolled up ({:.0} s)",
                i + 1,
                days.len(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    Ok(written)
}

/// Whole users from `next_user` on, about [`CHUNK`] events, never more than
/// `left`: the last user is cut short so a run has exactly the events asked.
fn realistic_chunk(
    next_user: &mut u64,
    left: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<openlens_event::Event> {
    let mut out = Vec::new();
    while (out.len() as u64) < CHUNK.min(left) {
        out.extend(openlens_testkit::realistic_user("default", *next_user, now));
        *next_user += 1;
    }
    out.truncate(usize::try_from(left).unwrap_or(usize::MAX));
    out
}
