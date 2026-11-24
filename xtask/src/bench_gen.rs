//! `cargo xtask bench-gen <data-dir> [--events N] [--users N]` (#321):
//! write a large synthetic app (63 M events over 90 days by default) into
//! a data dir, for scale benchmarks: start the server on it and measure
//! the board. Writes through the store's own writer in chunks, then
//! compacts each day, so the files look like a server's after 90 days.
//! Deterministic. A small run (`--events 1000000`) suits CI.

use std::time::Instant;

const CHUNK: u64 = 250_000;

pub(crate) fn bench_gen(args: &[String]) -> Result<(), String> {
    let usage = "usage: cargo xtask bench-gen <data-dir> [--events N] [--users N]";
    let dir = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or(usage)?
        .clone();
    let mut events: u64 = 63_000_000;
    let mut users: u64 = 200_000;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let v = it.next().ok_or(usage)?;
        let n = v.replace('_', "").parse::<u64>().map_err(|_| usage)?;
        match a.as_str() {
            "--events" => events = n,
            "--users" => users = n,
            _ => return Err(usage.into()),
        }
    }
    let (mut w, _) = openlens_storage::Writer::open(&dir, openlens_storage::FlushPolicy::default())
        .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let started = Instant::now();
    let mut done = 0;
    while done < events {
        let chunk = openlens_testkit::scale_chunk("default", events, users, done, CHUNK, now);
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
    println!(
        "{}",
        serde_json::json!({
            "events": events,
            "users": users,
            "days_compacted": days,
            "seconds": started.elapsed().as_secs(),
            "dir": dir,
        })
    );
    Ok(())
}
