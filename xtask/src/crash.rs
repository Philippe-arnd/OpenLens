//! `cargo xtask crash [--secs 300] [--kills 50] [--seed N] [--bin PATH]`:
//! the durability proof (#331).
//!
//! Starts the real server on a fresh data directory and ingests batches
//! with known uuids. It kills the server with SIGKILL at random intervals
//! (seeded, so a failure replays with the same `--seed`) and restarts it.
//! A batch that got no 200 is sent again with the same uuids after the
//! restart, as the SDK does. At the end it stops the server cleanly,
//! starts it once more, and reads the stored uuids through the SQL API.
//!
//! Passes when every acknowledged uuid is stored, no uuid is stored twice,
//! and the run reached `--kills` kills. Prints a JSON report either way.
//! Unix only (SIGKILL / SIGTERM).

use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const KEY: &str = "crash-key";
const TOKEN: &str = "crash-admin-token";
const BATCH: usize = 10;
/// Concurrent writers: more requests in flight when a kill lands.
const WRITERS: u64 = 4;
/// The ingestion rate limit is 50 requests/s per address: 4 writers at one
/// request every 100 ms stay under it.
const PAUSE: Duration = Duration::from_millis(100);

/// Seeded xorshift: no dependency, and the same seed gives the same kills.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `[lo, hi)` milliseconds.
    fn ms(&mut self, lo: u64, hi: u64) -> Duration {
        Duration::from_millis(lo + self.next() % (hi - lo).max(1))
    }
}

/// A valid UUID (v7 layout) for event number `n` of run `run`.
fn uuid(run: u32, n: u64) -> String {
    format!("{run:08x}-0000-7000-8000-{:012x}", n & 0xffff_ffff_ffff)
}

struct Opts {
    secs: u64,
    kills: u32,
    seed: u64,
    bin: PathBuf,
}

fn opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        secs: 300,
        kills: 50,
        seed: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64 | 1),
        bin: PathBuf::from("target/debug/openlens"),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--secs" => o.secs = val()?.parse().map_err(|_| "--secs: a number")?,
            "--kills" => o.kills = val()?.parse().map_err(|_| "--kills: a number")?,
            "--seed" => o.seed = val()?.parse::<u64>().map_err(|_| "--seed: a number")? | 1,
            "--bin" => o.bin = PathBuf::from(val()?),
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    Ok(o)
}

/// One HTTP/1.1 request on a fresh connection: (status, body).
fn http(
    addr: &str,
    method: &str,
    path: &str,
    auth: bool,
    body: &str,
) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let bearer = if auth {
        format!("Authorization: Bearer {TOKEN}\r\n")
    } else {
        String::new()
    };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\n{bearer}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    let status = out
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or("no status line")?;
    let body = out.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_owned();
    Ok((status, body))
}

fn start(o: &Opts, dir: &Path, addr: &str, log: &Path) -> Result<Child, String> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new(&o.bin)
        .env("OPENLENS_DATA_DIR", dir)
        .env("OPENLENS_ADDR", addr)
        .env("OPENLENS_ADMIN_TOKEN", TOKEN)
        .env("OPENLENS_PROJECT_KEY", KEY)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e} (build it first)", o.bin.display()))?;
    let up = Instant::now();
    while up.elapsed() < Duration::from_secs(60) {
        if matches!(http(addr, "GET", "/healthz", false, ""), Ok((200, _))) {
            return Ok(child);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("the server did not come up within 60 s".into())
}

fn batch(run: u32, first: u64) -> (String, Vec<String>) {
    let ids: Vec<String> = (first..first + BATCH as u64)
        .map(|n| uuid(run, n))
        .collect();
    let events: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(i, u)| {
            format!(
                r#"{{"event":"crash","distinct_id":"user-{}","uuid":"{u}"}}"#,
                i % 7
            )
        })
        .collect();
    (
        format!(r#"{{"key":"{KEY}","batch":[{}]}}"#, events.join(",")),
        ids,
    )
}

struct Written {
    acked: Vec<String>,
    sent: u64,
    retried: u64,
}

/// One writer: post batches until `end`, keeping the ones that got no 200
/// to resend with the same uuids; once the kills are over, deliver them.
fn writer(
    addr: &str,
    run: u32,
    w: u64,
    end: Instant,
    killed_all: &AtomicBool,
) -> Result<Written, String> {
    let mut out = Written {
        acked: Vec::new(),
        sent: 0,
        retried: 0,
    };
    let mut retry: VecDeque<(String, Vec<String>)> = VecDeque::new();
    // Writer `w` numbers its events from w × 10¹⁰: no overlap.
    let mut next = w * 10_000_000_000;
    let mut drained: Option<Instant> = None;
    loop {
        let draining = Instant::now() >= end;
        if draining {
            if !killed_all.load(Ordering::SeqCst) {
                std::thread::sleep(PAUSE);
                continue;
            }
            if retry.is_empty() {
                return Ok(out);
            }
            if drained.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(60) {
                return Err("unacknowledged batches still refused after 60 s".into());
            }
        }
        let (body, ids) = match retry.pop_front() {
            Some(b) => {
                out.retried += 1;
                b
            }
            None if draining => continue,
            None => {
                let b = batch(run, next);
                next += BATCH as u64;
                b
            }
        };
        out.sent += 1;
        match http(addr, "POST", "/capture", false, &body) {
            Ok((200, _)) => out.acked.extend(ids),
            _ => retry.push_back((body, ids)),
        }
        std::thread::sleep(PAUSE);
    }
}

/// Every stored uuid of the `default` project, paged under the row cap.
fn stored(addr: &str) -> Result<(u64, Vec<String>), String> {
    let q = |sql: String| -> Result<serde_json::Value, String> {
        let body = serde_json::json!({ "sql": sql }).to_string();
        let (s, b) = http(addr, "POST", "/api/projects/default/sql", true, &body)?;
        if s != 200 {
            return Err(format!("sql {s}: {b}"));
        }
        serde_json::from_str(&b).map_err(|e| e.to_string())
    };
    let total = q("SELECT count(*) AS n FROM raw_events WHERE event = 'crash'".into())?["rows"][0]
        ["n"]
        .as_u64()
        .unwrap_or(0);
    let mut ids = Vec::new();
    loop {
        let page = q(format!(
            "SELECT DISTINCT uuid FROM raw_events WHERE event = 'crash' ORDER BY uuid LIMIT 5000 OFFSET {}",
            ids.len()
        ))?;
        let rows = page["rows"].as_array().cloned().unwrap_or_default();
        let n = rows.len();
        ids.extend(
            rows.iter()
                .filter_map(|r| r["uuid"].as_str().map(str::to_owned)),
        );
        if n < 5000 {
            break;
        }
    }
    Ok((total, ids))
}

/// The verdict: acknowledged uuids missing from the store, and duplicates.
fn verdict(acked: &BTreeSet<String>, total: u64, distinct: &[String]) -> (Vec<String>, u64) {
    let have: BTreeSet<&str> = distinct.iter().map(String::as_str).collect();
    let missing = acked
        .iter()
        .filter(|u| !have.contains(u.as_str()))
        .cloned()
        .collect();
    (missing, total.saturating_sub(distinct.len() as u64))
}

pub(crate) fn crash(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let mut rng = Rng(o.seed);
    let run = (o.seed % 0xffff_ffff) as u32;
    let dir = std::env::temp_dir().join(format!("openlens-crash-{}", o.seed));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let log = dir.with_extension("log");
    let addr = format!("127.0.0.1:{}", 18_900 + o.seed % 90);
    eprintln!(
        "xtask: crash test, seed {} ({} s, at least {} kills), log {}",
        o.seed,
        o.secs,
        o.kills,
        log.display()
    );

    // Kill on average a little more often than `secs / kills`, so the run
    // reaches its kill count despite the restart time. Kills come from
    // their own thread while WRITERS post, so they land mid-request too:
    // that is what sends unacknowledged batches back and tests dedupe.
    let gap = (o.secs * 1000 / u64::from(o.kills.max(1))) * 3 / 4;
    let child = Mutex::new(start(&o, &dir, &addr, &log)?);
    let end = Instant::now() + Duration::from_secs(o.secs);
    let killed_all = AtomicBool::new(false);
    let (kills, writers) = std::thread::scope(|sc| {
        let killer = sc.spawn(|| -> Result<u32, String> {
            let mut kills = 0;
            loop {
                let at = Instant::now() + rng.ms(gap / 4, gap * 7 / 4);
                if at >= end {
                    break;
                }
                std::thread::sleep(at.saturating_duration_since(Instant::now()));
                let mut c = child.lock().map_err(|_| "lock")?;
                c.kill().map_err(|e| e.to_string())?; // SIGKILL
                let _ = c.wait();
                kills += 1;
                *c = start(&o, &dir, &addr, &log)?;
            }
            killed_all.store(true, Ordering::SeqCst);
            Ok(kills)
        });
        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let (addr, killed_all) = (&addr, &killed_all);
                sc.spawn(move || writer(addr, run, w, end, killed_all))
            })
            .collect();
        let kills = killer
            .join()
            .map_err(|_| "killer thread panicked".to_owned())
            .and_then(|r| r);
        if kills.is_err() {
            killed_all.store(true, Ordering::SeqCst);
        }
        let writers: Vec<_> = writers.into_iter().map(|t| t.join()).collect();
        (kills, writers)
    });
    let kills = kills?;
    let mut child = child.into_inner().map_err(|_| "lock")?;
    let (mut acked, mut sent, mut retried) = (BTreeSet::new(), 0u64, 0u64);
    for w in writers {
        let w = w.map_err(|_| "writer thread panicked".to_owned())??;
        acked.extend(w.acked);
        sent += w.sent;
        retried += w.retried;
    }

    // Stop cleanly, start once more, and read back.
    let _ = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    let _ = child.wait();
    child = start(&o, &dir, &addr, &log)?;
    let read = stored(&addr);
    let _ = child.kill();
    let _ = child.wait();
    let (total, distinct) = read?;
    let (missing, duplicates) = verdict(&acked, total, &distinct);
    let ok = missing.is_empty() && duplicates == 0 && kills >= o.kills;
    println!(
        "{}",
        serde_json::json!({
            "ok": ok,
            "seed": o.seed,
            "kills": kills,
            "batches_sent": sent,
            "batches_retried": retried,
            "acknowledged": acked.len(),
            "stored": total,
            "missing": missing.len(),
            "missing_sample": missing.iter().take(10).collect::<Vec<_>>(),
            "duplicates": duplicates,
        })
    );
    if ok {
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&log);
        Ok(())
    } else if kills < o.kills {
        Err(format!(
            "only {kills} kills (wanted {}); seed {}",
            o.kills, o.seed
        ))
    } else {
        Err(format!(
            "{} acknowledged events missing, {duplicates} duplicates; replay with --seed {} (data kept in {})",
            missing.len(),
            o.seed,
            dir.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    #[test]
    fn uuids_are_valid_and_distinct() {
        let a = super::uuid(7, 1);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'7', "version 7 layout");
        assert_ne!(super::uuid(7, 2), a);
    }

    #[test]
    fn the_verdict_finds_losses_and_duplicates() {
        let acked: BTreeSet<String> = ["a", "b", "c"].map(String::from).into();
        let stored = ["a".to_owned(), "c".to_owned(), "x".to_owned()];
        let (missing, dup) = super::verdict(&acked, 4, &stored);
        assert_eq!(missing, ["b"]);
        assert_eq!(dup, 1, "four rows, three distinct uuids");
    }
}
