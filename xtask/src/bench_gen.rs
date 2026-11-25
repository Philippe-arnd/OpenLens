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
//! 24-feature, 3-role feature map, to push before measuring.

use std::time::Instant;

const CHUNK: u64 = 250_000;

pub(crate) fn bench_gen(args: &[String]) -> Result<(), String> {
    let usage = "usage: cargo xtask bench-gen <data-dir> [--events N] [--users N] [--profile realistic] [--map-out FILE]";
    let dir = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or(usage)?
        .clone();
    let mut events: u64 = 63_000_000;
    let mut users: u64 = 200_000;
    let mut realistic = false;
    let mut map_out: Option<String> = None;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
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
    if let Some(path) = &map_out {
        std::fs::write(path, openlens_testkit::realistic_feature_map())
            .map_err(|e| e.to_string())?;
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
            "seconds": started.elapsed().as_secs(),
            "dir": dir,
        })
    );
    Ok(())
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
