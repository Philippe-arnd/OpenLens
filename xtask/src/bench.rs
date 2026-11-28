//! `cargo xtask bench <url> <key> [secs] [conns] [batch]`: ingestion load
//! test against a running server.
//!
//! Plain HTTP/1.1 keep-alive over `std::net` (no client dependency), one
//! thread per connection, each posting batches of realistic SDK events to
//! `/capture` as fast as the server answers. Prints accepted events per
//! second and latency percentiles as JSON. Only `http://` (local) targets.
//!
//! `--with-board PROJECT` (#344) also computes that project's board in a
//! loop on two more connections, as two people reading the dashboard
//! would, to see what reads cost ingestion. It needs a dashboard session:
//! put the `ol_session=…` cookie in `OPENLENS_BENCH_COOKIE`. The capture
//! p99 should stay under 50 ms; above, the bench warns (it does not fail
//! until a reference machine is defined, #314).
//!
//! `--rate N` paces the senders to N events/s in all instead of as fast as
//! the server answers: a steady load near what a real app sends, where the
//! board is recomputed on fresh data instead of served from its cache.
//! With `--with-board`, each reader then loads one page every 5 s, as a
//! person browsing would. With `OPENLENS_BENCH_TOKEN` (the admin token), it
//! also measures **time to visible**: one marker event is sent after a
//! warm-up, then `/api/projects/PROJECT/sql` is polled until the event is
//! readable (`visible_s`), then `/api/projects/PROJECT/board` until its
//! answer changes (`board_refreshed_s`: about when the board shows data
//! sent at that moment, since any board computed after the event became
//! readable includes it).

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

/// GET `path` with the session cookie on a fresh connection; the status.
fn get_page(host: &str, path: &str, cookie: &str) -> Result<u16, String> {
    let mut s = TcpStream::connect(host).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out)
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| "no status line".to_owned())
}

/// One request on a fresh connection: (status, body). For the JSON API,
/// which answers with a content length, so the body is the whole JSON.
fn request(
    host: &str,
    method: &str,
    path: &str,
    token: &str,
    body: &str,
) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(host).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| "no status line".to_owned())?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    Ok((status, body))
}

/// Time to visible (`--rate` with `OPENLENS_BENCH_TOKEN`): after `warmup`,
/// one marker event, then the seconds until the query layer reads it and
/// until the board's answer changes. `None` for a step that did not happen
/// before `until`.
fn time_to_visible(
    host: &str,
    key: &str,
    project: &str,
    token: &str,
    warmup: Duration,
    until: Instant,
) -> Result<(Option<f64>, Option<f64>), String> {
    std::thread::sleep(warmup);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let who = format!("bench-marker-{nanos}");
    let body = format!(
        r#"{{"key":"{key}","batch":[{{"event":"$pageview","distinct_id":"{who}","session_id":"{who}","properties":{{"$current_url":"https://app.example.com/marker","$environment":"production","$release":"v1.4.2","$role":"member","$lib":"web","$pathname":"/marker"}}}}]}}"#
    );
    let sent = Instant::now();
    let mut conn = BufReader::new(TcpStream::connect(host).map_err(|e| e.to_string())?);
    if post(&mut conn, host, &body)? != 200 {
        return Err("the marker event was refused".to_owned());
    }
    let sql = serde_json::json!({
        "sql": format!("SELECT count(*) AS n FROM events WHERE distinct_id = '{who}'")
    })
    .to_string();
    let mut visible = None;
    while Instant::now() < until {
        let (status, body) = request(
            host,
            "POST",
            &format!("/api/projects/{project}/sql"),
            token,
            &sql,
        )?;
        if status == 401 {
            return Err("OPENLENS_BENCH_TOKEN is not the admin token".to_owned());
        }
        let n = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("rows")?.get(0)?.get("n")?.as_i64())
            .unwrap_or(0);
        if n > 0 {
            visible = Some(sent.elapsed().as_secs_f64());
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let Some(v) = visible else {
        return Ok((None, None));
    };
    let board_path = format!("/api/projects/{project}/board");
    let (_, before) = request(host, "GET", &board_path, token, "")?;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(500));
        let (status, now) = request(host, "GET", &board_path, token, "")?;
        if status == 200 && now != before {
            return Ok((Some(v), Some(sent.elapsed().as_secs_f64())));
        }
    }
    Ok((Some(v), None))
}

/// Two readers computing `project`'s board until `deadline`: (pages, ms
/// each). `pace`: the time between two pages of one reader (`--rate`), or
/// none to load the next page as soon as one is done.
fn boards(
    host: &str,
    project: &str,
    cookie: &str,
    deadline: Instant,
    pace: Option<Duration>,
) -> Result<Vec<u128>, String> {
    let readers: Vec<_> = (0..2)
        .map(|r| {
            let (host, cookie) = (host.to_owned(), cookie.to_owned());
            // Alternate filters, so the board cache (#291) does not answer
            // every request from memory.
            let paths = [
                format!("/p/{project}"),
                format!("/p/{project}?env=production"),
                format!("/p/{project}?role=member"),
            ];
            std::thread::spawn(move || -> Result<Vec<u128>, String> {
                let mut ms = Vec::new();
                let mut i = r;
                while Instant::now() < deadline {
                    let t = Instant::now();
                    let status = get_page(&host, &paths[i % paths.len()], &cookie)?;
                    if status != 200 {
                        return Err(format!(
                            "board answered {status}: is OPENLENS_BENCH_COOKIE a valid session?"
                        ));
                    }
                    ms.push(t.elapsed().as_millis());
                    i += 1;
                    if let Some(p) = pace {
                        let next = t + p;
                        let now = Instant::now();
                        if next > now && next < deadline {
                            std::thread::sleep(next - now);
                        }
                    }
                }
                Ok(ms)
            })
        })
        .collect();
    let mut all = Vec::new();
    for r in readers {
        all.extend(r.join().map_err(|_| "board reader crashed")??);
    }
    Ok(all)
}

pub(crate) fn bench(args: &[String]) -> Result<(), String> {
    let usage = "usage: cargo xtask bench http://HOST:PORT KEY [secs=60] [conns=4] [batch=50] [--with-board PROJECT] [--rate EVENTS_PER_SEC]";
    let mut args = args.to_vec();
    let mut flag = |name: &str| -> Result<Option<String>, String> {
        match args.iter().position(|a| a == name) {
            Some(i) => {
                let v = args.get(i + 1).ok_or(usage)?.clone();
                args.drain(i..=i + 1);
                Ok(Some(v))
            }
            None => Ok(None),
        }
    };
    let board_project = flag("--with-board")?;
    let rate: Option<f64> = match flag("--rate")? {
        Some(r) => Some(
            r.parse::<f64>()
                .ok()
                .filter(|r| *r >= 1.0)
                .ok_or("--rate needs a number of events per second, at least 1")?,
        ),
        None => None,
    };
    let args = &args;
    let url = args.first().ok_or(usage)?;
    let key = args.get(1).ok_or(usage)?.clone();
    let num = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (secs, conns, batch) = (num(2, 60), num(3, 4) as usize, num(4, 50) as usize);
    let host = url
        .strip_prefix("http://")
        .ok_or("only http:// targets (a local server)")?
        .trim_end_matches('/')
        .to_owned();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs);
    // Each sender posts one batch every `interval`, so all of them together
    // send `rate` events a second.
    let interval = rate.map(|r| Duration::from_secs_f64((conns * batch) as f64 / r));
    let pace = rate.map(|_| Duration::from_secs(5));
    let board = match &board_project {
        Some(project) => {
            let cookie = std::env::var("OPENLENS_BENCH_COOKIE")
                .map_err(|_| "--with-board needs OPENLENS_BENCH_COOKIE (ol_session=…)")?;
            let (host, project) = (host.clone(), project.clone());
            Some(std::thread::spawn(move || {
                boards(&host, &project, &cookie, deadline, pace)
            }))
        }
        None => None,
    };
    // Time to visible: a paced run with the admin token. The marker goes
    // out after a third of the run (at most 20 s), and the wait for it may
    // run 60 s past the end of the load.
    let visible = match (rate, std::env::var("OPENLENS_BENCH_TOKEN")) {
        (Some(_), Ok(token)) => {
            let (host, key) = (host.clone(), key.clone());
            let project = board_project
                .clone()
                .unwrap_or_else(|| "default".to_owned());
            let warmup = Duration::from_secs((secs / 3).min(20));
            let until = deadline + Duration::from_secs(60);
            Some(std::thread::spawn(move || {
                time_to_visible(&host, &key, &project, &token, warmup, until)
            }))
        }
        _ => None,
    };
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
                    if let Some(every) = interval {
                        let next = start + every.mul_f64(i as f64);
                        let now = Instant::now();
                        if next > now {
                            std::thread::sleep(
                                (next - now).min(deadline.saturating_duration_since(now)),
                            );
                        }
                    }
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
    let board = match board {
        Some(t) => {
            let mut ms = t.join().map_err(|_| "board readers crashed")??;
            ms.sort_unstable();
            let q = |f: f64| {
                ms.get(((ms.len() as f64 * f) as usize).min(ms.len().saturating_sub(1)))
                    .copied()
                    .unwrap_or(0)
            };
            Some(serde_json::json!({ "pages": ms.len(), "ms": { "p50": q(0.5), "max": q(1.0) } }))
        }
        None => None,
    };
    let (visible_s, board_refreshed_s) = match visible {
        Some(t) => t.join().map_err(|_| "time-to-visible probe crashed")??,
        None => (None, None),
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
            "board": board,
            "rate": rate,
            "visible_s": visible_s,
            "board_refreshed_s": board_refreshed_s,
        })
    );
    if board.is_some() && p(0.99) > 50.0 {
        eprintln!(
            "xtask: warning: capture p99 is {:.1} ms with two boards computing (target: under 50 ms, #344)",
            p(0.99)
        );
    }
    if failed > 0 {
        return Err(format!("{failed} requests failed"));
    }
    Ok(())
}
