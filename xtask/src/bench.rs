//! `cargo xtask bench <url> <key> [secs] [conns] [batch]`: ingestion load
//! test against a running server.
//!
//! Plain HTTP/1.1 keep-alive over `std::net` (no client dependency), one
//! thread per connection, each posting batches of realistic SDK events to
//! `/capture` as fast as the server answers. Prints accepted events per
//! second and latency percentiles as JSON. Only `http://` (local) targets.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn batch_body(key: &str, n: usize, seed: usize) -> String {
    let events: Vec<String> = (0..n)
        .map(|i| {
            let u = (seed * 7919 + i) % 5000;
            let (event, extra) = match i % 5 {
                0 => ("$pageview", r#""$pathname":"/projects/42/board""#),
                1 => ("$click", r#""$tag":"button","$text":"Export CSV","$feature":"export""#),
                2 => ("$vital", r#""$name":"LCP","$value":2380.5,"$rating":"good""#),
                3 => ("$feature", r#""$feature":"reports""#),
                _ => ("$submit", r#""$form":"invite""#),
            };
            // Realistic identities (#159): a third of the users browse
            // anonymously before logging in, and `$identify` links that
            // visit; one event in seven comes from a visitor who never logs in.
            let (event, extra, who) = if i % 7 == 6 {
                (event, extra.to_owned(), format!("visitor-{seed}-{i}"))
            } else if u.is_multiple_of(3) && i % 5 == 4 {
                (
                    "$identify",
                    format!(r#""$anon_distinct_id":"anon-{u}-{}""#, seed % 8),
                    format!("user-{u}"),
                )
            } else if u.is_multiple_of(3) && seed.is_multiple_of(2) {
                (event, extra.to_owned(), format!("anon-{u}-{}", seed % 8))
            } else {
                (event, extra.to_owned(), format!("user-{u}"))
            };
            format!(
                r#"{{"event":"{event}","distinct_id":"{who}","session_id":"s-{u}-{seed}","properties":{{"$current_url":"https://app.example.com/projects/42","$environment":"production","$release":"v1.4.2","$role":"member","$lib":"web",{extra}}}}}"#
            )
        })
        .collect();
    format!(r#"{{"key":"{key}","batch":[{}]}}"#, events.join(","))
}

/// Send one request on a keep-alive connection; returns the status code.
fn post(conn: &mut BufReader<TcpStream>, host: &str, body: &str) -> Result<u16, String> {
    let req = format!(
        "POST /capture HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    conn.get_mut()
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    conn.read_line(&mut line).map_err(|e| e.to_string())?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line {line:?}"))?;
    let mut len = 0usize;
    loop {
        line.clear();
        conn.read_line(&mut line).map_err(|e| e.to_string())?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(str::trim)
            .map(str::to_owned)
        {
            len = v.parse().unwrap_or(0);
        }
    }
    let mut sink = vec![0u8; len];
    conn.read_exact(&mut sink).map_err(|e| e.to_string())?;
    Ok(status)
}

pub(crate) fn bench(args: &[String]) -> Result<(), String> {
    let usage = "usage: cargo xtask bench http://HOST:PORT KEY [secs=60] [conns=4] [batch=50]";
    let url = args.first().ok_or(usage)?;
    let key = args.get(1).ok_or(usage)?.clone();
    let num = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (secs, conns, batch) = (num(2, 60), num(3, 4) as usize, num(4, 50) as usize);
    let host = url
        .strip_prefix("http://")
        .ok_or("only http:// targets (a local server)")?
        .trim_end_matches('/')
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(secs);
    let workers: Vec<_> = (0..conns)
        .map(|c| {
            let (host, key) = (host.clone(), key.clone());
            std::thread::spawn(move || -> Result<(usize, usize, Vec<u128>), String> {
                let stream = TcpStream::connect(&host).map_err(|e| e.to_string())?;
                stream.set_nodelay(true).map_err(|e| e.to_string())?;
                let mut conn = BufReader::new(stream);
                let bodies: Vec<String> = (0..16)
                    .map(|i| batch_body(&key, batch, c * 16 + i))
                    .collect();
                let (mut ok, mut failed, mut lat) = (0usize, 0usize, Vec::new());
                let mut i = 0;
                while Instant::now() < deadline {
                    let t = Instant::now();
                    match post(&mut conn, &host, &bodies[i % bodies.len()])? {
                        200 => ok += batch,
                        _ => failed += 1,
                    }
                    lat.push(t.elapsed().as_micros());
                    i += 1;
                }
                Ok((ok, failed, lat))
            })
        })
        .collect();
    let (mut ok, mut failed, mut lat) = (0usize, 0usize, Vec::new());
    for w in workers {
        let (o, f, l) = w.join().map_err(|_| "worker crashed")??;
        ok += o;
        failed += f;
        lat.extend(l);
    }
    lat.sort_unstable();
    let p = |q: f64| {
        lat.get(((lat.len() as f64 * q) as usize).min(lat.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0) as f64
            / 1000.0
    };
    println!(
        "{}",
        serde_json::json!({
            "events_per_sec": (ok as f64 / secs as f64).round(),
            "requests": lat.len(),
            "failed_requests": failed,
            "batch": batch,
            "connections": conns,
            "seconds": secs,
            "latency_ms": { "p50": p(0.5), "p95": p(0.95), "p99": p(0.99), "p99_9": p(0.999), "max": p(1.0) },
        })
    );
    if failed > 0 {
        return Err(format!("{failed} requests failed"));
    }
    Ok(())
}
