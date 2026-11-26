//! The cold tier (#326, D-031): with S3 set, an event day older than
//! [`COLD_AFTER_DAYS`] lives on S3 only. A day leaves the disk only once
//! its rollups exist and S3 holds every one of its files at the same size;
//! a marker `cold/project=<p>/date=<d>` says where it went. Erasing someone
//! brings each cold day back, erases them there and sends it up again, and
//! a person's export brings the project's cold days back first. S3 objects
//! of cold days older than the 400-day retention are deleted (D-015).
//!
//! Every function here runs in the background S3 job or an admin request,
//! with the maintenance lock held by the caller.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::s3::{self, Config};

/// Days kept on disk when S3 is set: past every window the dashboard
/// reads raw events for (goals read 112 days).
pub(crate) const COLD_AFTER_DAYS: i64 = 120;

/// The shortest bucket expiration the tier accepts (#417): the 400-day
/// retention, so S3 never drops a day OpenLens still promises to keep.
pub(crate) const MIN_BUCKET_EXPIRY_DAYS: u32 = 400;

/// The owner's opt-in (#417, D-032): off by default. `OPENLENS_S3_COLD`
/// (`on` / `off`) wins over the Storage page.
pub(crate) const OPT_IN: &str = "s3_cold";
/// Why no day moves although the tier is on (empty: nothing in the way).
pub(crate) const BLOCKED: &str = "s3_cold_blocked";

/// Is the cold tier on?
pub(crate) fn enabled(meta: &openlens_meta::Meta) -> bool {
    match std::env::var("OPENLENS_S3_COLD")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("on") => true,
        Some("off") => false,
        _ => meta.setting(OPT_IN).ok().flatten().as_deref() == Some("on"),
    }
}

/// Is the opt-in set by the environment (read-only on the page)?
pub(crate) fn from_env() -> bool {
    std::env::var("OPENLENS_S3_COLD").is_ok_and(|v| matches!(v.trim(), "on" | "off"))
}

/// Why the tier is on but moves nothing, if it is blocked.
pub(crate) fn blocked(meta: &openlens_meta::Meta) -> Option<String> {
    meta.setting(BLOCKED)
        .ok()
        .flatten()
        .filter(|v| !v.is_empty())
}

/// [`offload`], only when the owner opted in and the bucket keeps objects
/// long enough (#417). The bucket's lifecycle rules are read first; when
/// they cannot be read, nothing moves (fail closed). The reason is kept for
/// the Storage page and `/metrics`.
pub(crate) fn offload_checked(
    cfg: &Config,
    root: &Path,
    meta: &openlens_meta::Meta,
    today: NaiveDate,
) -> Result<usize, String> {
    if !enabled(meta) {
        let _ = meta.set_setting(BLOCKED, "");
        return Ok(0);
    }
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    ANNOUNCED.call_once(|| {
        tracing::warn!(
            "cold tier on: event days older than {COLD_AFTER_DAYS} days live only on S3 (D-031, D-032)"
        );
    });
    let reason = match s3::min_expiration_days(cfg) {
        Ok(Some(d)) if d < MIN_BUCKET_EXPIRY_DAYS => Some(format!(
            "the bucket expires objects after {d} days, under the {MIN_BUCKET_EXPIRY_DAYS}-day retention: no day leaves the disk"
        )),
        Ok(_) => None,
        Err(e) => Some(format!(
            "the bucket's lifecycle rules cannot be read ({e}): no day leaves the disk"
        )),
    };
    let _ = meta.set_setting(BLOCKED, reason.as_deref().unwrap_or(""));
    if let Some(r) = reason {
        tracing::warn!(reason = %r, "cold tier blocked");
        return Ok(0);
    }
    offload(cfg, root, today)
}

/// The upload mark `s3::sync` leaves in a day directory.
const MARK: &str = ".s3-uploaded";

fn markers(root: &Path) -> PathBuf {
    root.join("cold")
}

/// `(project, day)` of every cold day.
pub(crate) fn cold_days(root: &Path) -> Vec<(String, NaiveDate)> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(markers(root)) else {
        return out;
    };
    for p in projects.flatten() {
        let Some(project) = p
            .file_name()
            .to_str()
            .and_then(|n| n.strip_prefix("project="))
            .map(str::to_owned)
        else {
            continue;
        };
        let Ok(days) = fs::read_dir(p.path()) else {
            continue;
        };
        for d in days.flatten() {
            if let Some(day) = day_of(&d.file_name().to_string_lossy()) {
                out.push((project.clone(), day));
            }
        }
    }
    out.sort();
    out
}

fn day_of(name: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(name.strip_prefix("date=")?, "%Y-%m-%d").ok()
}

/// `(project, day)` of every cold day, found on S3 itself rather than
/// from the local markers (NSEC-02): S3 is the source of truth, since a
/// restore or a disk loss leaves the markers behind but not the bucket.
fn s3_cold_days(cfg: &Config) -> Result<Vec<(String, NaiveDate)>, String> {
    let mut out = Vec::new();
    for p in s3::list_prefixes(cfg, "events/")? {
        let Some(project) = p
            .strip_prefix("events/project=")
            .map(|s| s.trim_end_matches('/').to_owned())
        else {
            continue;
        };
        for d in s3::list_prefixes(cfg, &p)? {
            let Some(day) = d.rsplit('/').find(|s| !s.is_empty()).and_then(day_of) else {
                continue;
            };
            out.push((project.clone(), day));
        }
    }
    out.sort();
    Ok(out)
}

/// `(project, day)` of every cold day: the union of the local markers and
/// what S3 itself lists, so a day whose marker was lost (a restore, or the
/// disk failing while the bucket survived) is still found (NSEC-02). Any
/// day S3 has but the markers do not is written back as a marker, so the
/// markers stay a cache of this, rebuilt as they are read rather than only
/// at startup.
pub(crate) fn authoritative_cold_days(
    cfg: &Config,
    root: &Path,
) -> Result<Vec<(String, NaiveDate)>, String> {
    let local: HashSet<(String, NaiveDate)> = cold_days(root).into_iter().collect();
    let remote = s3_cold_days(cfg)?;
    for (project, day) in &remote {
        // Already known cold, or still a live local day (S3 also holds
        // every recent day as a plain backup copy, D-016): neither needs
        // its marker rebuilt.
        if local.contains(&(project.clone(), *day)) || root.join(rel(project, *day)).is_dir() {
            continue;
        }
        let names = s3::list(cfg, &format!("{}/", rel(project, *day)))?
            .into_iter()
            .filter_map(|k| k.rsplit('/').next().map(str::to_owned))
            .filter(|n| n.ends_with(".parquet"))
            .map(|n| n + "\n")
            .collect::<String>();
        let m = marker(root, project, *day);
        if let Some(parent) = m.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&m, names).map_err(|e| e.to_string())?;
        tracing::info!(%project, %day, "cold day marker rebuilt from S3");
    }
    Ok(cold_days(root))
}

/// The S3 key prefix of a cold day (also written to backup manifests).
pub(crate) fn rel(project: &str, day: NaiveDate) -> String {
    format!("events/project={project}/date={}", day.format("%Y-%m-%d"))
}

fn marker(root: &Path, project: &str, day: NaiveDate) -> PathBuf {
    markers(root)
        .join(format!("project={project}"))
        .join(format!("date={}", day.format("%Y-%m-%d")))
}

/// The Parquet files of a local day, sorted.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut f: Vec<PathBuf> = fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
                .collect()
        })
        .unwrap_or_default();
    f.sort();
    f
}

fn listing(files: &[PathBuf]) -> String {
    files
        .iter()
        .filter_map(|f| {
            f.file_name()
                .map(|n| n.to_string_lossy().into_owned() + "\n")
        })
        .collect()
}

/// Move the closed days older than [`COLD_AFTER_DAYS`] to S3: only a day
/// whose rollups exist, whose upload mark matches its files, and whose
/// every file S3 lists at the same size. Returns the days moved.
pub(crate) fn offload(cfg: &Config, root: &Path, today: NaiveDate) -> Result<usize, String> {
    let cutoff = today - chrono::Duration::days(COLD_AFTER_DAYS);
    let mut moved = 0;
    let Ok(projects) = fs::read_dir(root.join("events")) else {
        return Ok(0);
    };
    for p in projects.flatten() {
        let Some(project) = p
            .file_name()
            .to_str()
            .and_then(|n| n.strip_prefix("project="))
            .map(str::to_owned)
        else {
            continue;
        };
        let Ok(days) = fs::read_dir(p.path()) else {
            continue;
        };
        for d in days.flatten() {
            let Some(day) = day_of(&d.file_name().to_string_lossy()) else {
                continue;
            };
            let dir = d.path();
            if day >= cutoff || !openlens_storage::rollup::has(root, &project, day).unwrap_or(false)
            {
                continue;
            }
            let local = files(&dir);
            if local.is_empty()
                || fs::read_to_string(dir.join(MARK)).ok().as_deref() != Some(&listing(&local))
            {
                continue;
            }
            let remote = s3::list_sized(cfg, &format!("{}/", rel(&project, day)))?;
            let verified = local.iter().all(|f| {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned());
                let size = fs::metadata(f).map(|m| m.len()).ok();
                name.zip(size).is_some_and(|(n, s)| {
                    remote
                        .iter()
                        .any(|(k, rs)| *k == format!("{}/{n}", rel(&project, day)) && *rs == s)
                })
            });
            if !verified {
                tracing::warn!(%project, %day, "S3 does not hold this day yet; kept on disk");
                continue;
            }
            let m = marker(root, &project, day);
            let Some(parent) = m.parent() else { continue };
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            fs::write(&m, listing(&local)).map_err(|e| e.to_string())?;
            // NSEC-02: the marker must survive a crash before the local
            // day is removed, or the day is lost from the markers (and,
            // until the next S3 listing rebuilds them, from erasure and
            // expiry) while its disk copy is already gone.
            openlens_storage::sync_dir(parent).map_err(|e| e.to_string())?;
            fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            moved += 1;
        }
    }
    Ok(moved)
}

/// The cold tier, after a good S3 copy (#326, D-031): people forgotten
/// since the last pass are erased from cold days too, then old days move
/// to S3 and cold days past retention go. Returns whether a day was
/// changed, moved or removed (#516).
pub(crate) fn run(cfg: &Config, root: &Path, meta: &openlens_meta::Meta) -> bool {
    const DONE: &str = "cold_erased_at";
    let Ok(_guard) = crate::privacy::MAINTENANCE.lock() else {
        return false;
    };
    let mut changed = false;
    let now = chrono::Utc::now();
    for p in meta.projects().unwrap_or_default() {
        let since = meta
            .project_setting(&p.id, DONE)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let ids: std::collections::HashSet<String> = meta
            .forgotten_since(&p.id, since)
            .unwrap_or_default()
            .into_iter()
            .collect();
        if !ids.is_empty() {
            match erase(cfg, root, &p.id, &ids) {
                Ok(n) => {
                    changed |= n > 0;
                    tracing::info!(project = %p.id, events = n, "erased from cold days");
                }
                Err(err) => {
                    tracing::warn!(project = %p.id, %err, "cold erasure failed; retried next hour");
                    continue;
                }
            }
        }
        let _ = meta.set_project_setting(&p.id, DONE, &now.timestamp().to_string());
    }
    match offload_checked(cfg, root, meta, now.date_naive()) {
        Ok(0) => {}
        Ok(n) => {
            changed = true;
            tracing::info!(days = n, "moved old event days to S3");
        }
        Err(err) => tracing::warn!(%err, "cold tier: move to S3 failed"),
    }
    match expire(cfg, root, now.date_naive()) {
        Ok(0) => {}
        Ok(n) => {
            changed = true;
            tracing::info!(days = n, "expired cold days on S3");
        }
        Err(err) => tracing::warn!(%err, "cold tier: expiry failed"),
    }
    changed
}

/// Bring a cold day back to the disk, as it was uploaded. Returns the
/// files fetched.
pub(crate) fn restore(
    cfg: &Config,
    root: &Path,
    project: &str,
    day: NaiveDate,
) -> Result<usize, String> {
    let prefix = format!("{}/", rel(project, day));
    let dir = root.join(rel(project, day));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for (key, _) in s3::list_sized(cfg, &prefix)? {
        let Some(name) = key
            .strip_prefix(&prefix)
            .filter(|n| n.ends_with(".parquet"))
        else {
            continue;
        };
        let body = s3::get(cfg, &key)?;
        let tmp = dir.join(format!(".{name}.tmp"));
        fs::write(&tmp, body).map_err(|e| e.to_string())?;
        fs::rename(&tmp, dir.join(name)).map_err(|e| e.to_string())?;
        names.push(name.to_owned());
    }
    // Uploaded as it is: the copy job need not send it again.
    fs::write(dir.join(MARK), listing(&files(&dir))).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(marker(root, project, day));
    Ok(names.len())
}

/// The bytes the cold days of `project` hold on S3.
pub(crate) fn project_bytes(cfg: &Config, root: &Path, project: &str) -> Result<u64, String> {
    let mut n = 0;
    for (p, day) in authoritative_cold_days(cfg, root)? {
        if p == project {
            n += s3::list_sized(cfg, &format!("{}/", rel(&p, day)))?
                .iter()
                .map(|(_, s)| s)
                .sum::<u64>();
        }
    }
    Ok(n)
}

/// Every cold day of `project` back on disk (a person's export reads
/// them). They go back to S3 at the next hourly pass.
pub(crate) fn restore_project(cfg: &Config, root: &Path, project: &str) -> Result<usize, String> {
    let mut n = 0;
    for (p, day) in authoritative_cold_days(cfg, root)? {
        if p == project {
            restore(cfg, root, &p, day)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Erase `ids` from the cold days of `project`: each comes back, is
/// rewritten without them, and goes up again (new files first, then the
/// old objects are deleted). Returns the events removed.
pub(crate) fn erase(
    cfg: &Config,
    root: &Path,
    project: &str,
    ids: &HashSet<String>,
) -> Result<usize, String> {
    let mut removed = 0;
    for (p, day) in authoritative_cold_days(cfg, root)? {
        if p != project {
            continue;
        }
        let before = s3::list_sized(cfg, &format!("{}/", rel(&p, day)))?;
        restore(cfg, root, &p, day)?;
        let n = openlens_storage::erase_day(root, &p, day, ids).map_err(|e| e.to_string())?;
        let dir = root.join(rel(&p, day));
        if n > 0 {
            let now = files(&dir);
            for f in &now {
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                s3::put(
                    cfg,
                    &format!("{}/{name}", rel(&p, day)),
                    &fs::read(f).map_err(|e| e.to_string())?,
                )?;
            }
            let kept: HashSet<String> = now
                .iter()
                .filter_map(|f| {
                    f.file_name()
                        .map(|n| format!("{}/{}", rel(&p, day), n.to_string_lossy()))
                })
                .collect();
            for (key, _) in before {
                if !kept.contains(&key) {
                    s3::delete(cfg, &key)?;
                }
            }
            removed += n;
        }
        // Back to the cold tier at once: S3 now holds exactly these files.
        let m = marker(root, &p, day);
        let Some(parent) = m.parent() else { continue };
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        fs::write(&m, listing(&files(&dir))).map_err(|e| e.to_string())?;
        openlens_storage::sync_dir(parent).map_err(|e| e.to_string())?;
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

/// Delete the S3 objects of cold days past the event retention (D-015).
/// Returns the days expired.
pub(crate) fn expire(cfg: &Config, root: &Path, today: NaiveDate) -> Result<usize, String> {
    let cutoff = today - chrono::Duration::days(openlens_storage::EVENT_RETENTION_DAYS);
    let mut n = 0;
    for (p, day) in authoritative_cold_days(cfg, root)? {
        if day >= cutoff {
            continue;
        }
        for (key, _) in s3::list_sized(cfg, &format!("{}/", rel(&p, day)))? {
            s3::delete(cfg, &key)?;
        }
        let _ = fs::remove_file(marker(root, &p, day));
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    type Store = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;

    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    /// An S3 endpoint that keeps objects: PUT, GET, DELETE and list-type=2.
    fn fake_store() -> (String, Store) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let store: Store = Arc::default();
        let st = store.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s);
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or("").to_owned();
                let target = parts.next().unwrap_or("").to_owned();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if h.trim().is_empty() {
                        break;
                    }
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                // `/bucket/key`.
                let key = decode(path.splitn(3, '/').nth(2).unwrap_or(""));
                let mut store = st.lock().unwrap();
                let (code, out): (&str, Vec<u8>) = match method.as_str() {
                    "PUT" => {
                        store.insert(key, body);
                        ("200 OK", Vec::new())
                    }
                    "DELETE" => {
                        store.remove(&key);
                        ("204 No Content", Vec::new())
                    }
                    "GET" if query.starts_with("lifecycle") => match store.get("__lifecycle__") {
                        Some(v) => ("200 OK", v.clone()),
                        None => (
                            "404 Not Found",
                            b"<Code>NoSuchLifecycleConfiguration</Code>".to_vec(),
                        ),
                    },
                    // A real bucket lists only at its root (`/bucket`).
                    "GET" if query.contains("list-type=2") && !key.is_empty() => {
                        ("404 Not Found", b"<Code>NoSuchKey</Code>".to_vec())
                    }
                    "GET" if query.contains("list-type=2") && query.contains("delimiter=") => {
                        let prefix = query
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("prefix="))
                            .map(decode)
                            .unwrap_or_default();
                        let mut seen = std::collections::BTreeSet::new();
                        for k in store.keys().filter(|k| k.starts_with(&prefix)) {
                            if let Some(rest) = k.get(prefix.len()..)
                                && let Some(i) = rest.find('/')
                            {
                                seen.insert(format!("{prefix}{}", &rest[..=i]));
                            }
                        }
                        let items: String = seen
                            .iter()
                            .map(|p| {
                                format!("<CommonPrefixes><Prefix>{p}</Prefix></CommonPrefixes>")
                            })
                            .collect();
                        (
                            "200 OK",
                            format!("<ListBucketResult>{items}</ListBucketResult>").into_bytes(),
                        )
                    }
                    "GET" if query.contains("list-type=2") => {
                        let prefix = query
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("prefix="))
                            .map(decode)
                            .unwrap_or_default();
                        let items: String = store
                            .iter()
                            .filter(|(k, _)| k.starts_with(&prefix))
                            .map(|(k, v)| {
                                format!(
                                    "<Contents><Key>{k}</Key><Size>{}</Size></Contents>",
                                    v.len()
                                )
                            })
                            .collect();
                        (
                            "200 OK",
                            format!("<ListBucketResult>{items}</ListBucketResult>").into_bytes(),
                        )
                    }
                    _ => match store.get(&key) {
                        Some(v) => ("200 OK", v.clone()),
                        None => ("404 Not Found", b"<Code>NoSuchKey</Code>".to_vec()),
                    },
                };
                let head = format!(
                    "HTTP/1.1 {code}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    out.len()
                );
                let w = r.get_mut();
                w.write_all(head.as_bytes()).unwrap();
                w.write_all(&out).unwrap();
            }
        });
        (url, store)
    }

    fn cfg(url: String) -> Config {
        Config {
            bucket: "lens".into(),
            region: "auto".into(),
            endpoint: Some(url),
            prefix: "prod/".into(),
            access_key: "ak".into(),
            secret_key: "sk".into(),
        }
    }

    fn event(day: NaiveDate, who: &str, n: u32) -> openlens_event::Event {
        let t = day.and_hms_opt(10, 0, n).unwrap().and_utc();
        openlens_event::Event {
            uuid: format!("0191f0c0-0000-7000-8000-{n:012}"),
            project: "p".into(),
            event: "$pageview".into(),
            distinct_id: who.into(),
            session_id: None,
            timestamp: t,
            received_at: t,
            url: None,
            path: None,
            feature: None,
            role: None,
            account: None,
            plan: None,
            environment: "production".into(),
            release: None,
            flags: None,
            properties: "{}".into(),
        }
    }

    #[test]
    fn lifecycle_rules_are_read_safely() {
        let r = |days: &str, status: &str| {
            format!(
                "<Rule><ID>x</ID><Status>{status}</Status><Expiration>{days}</Expiration></Rule>"
            )
        };
        let doc = |rules: &[String]| {
            format!(
                "<LifecycleConfiguration>{}</LifecycleConfiguration>",
                rules.concat()
            )
        };
        assert_eq!(s3::parse_min_expiration(&doc(&[])), None);
        assert_eq!(
            s3::parse_min_expiration(&doc(&[r("<Days>30</Days>", "Enabled")])),
            Some(30)
        );
        assert_eq!(
            s3::parse_min_expiration(&doc(&[
                r("<Days>30</Days>", "Disabled"),
                r("<Days>500</Days>", "Enabled")
            ])),
            Some(500)
        );
        assert_eq!(
            s3::parse_min_expiration(&doc(&[r("<Date>2027-01-01T00:00:00Z</Date>", "Enabled")])),
            Some(0)
        );
        let noncurrent = "<LifecycleConfiguration><Rule><Status>Enabled</Status><NoncurrentVersionExpiration><NoncurrentDays>7</NoncurrentDays></NoncurrentVersionExpiration></Rule></LifecycleConfiguration>";
        assert_eq!(
            s3::parse_min_expiration(noncurrent),
            None,
            "old versions only"
        );
    }

    #[test]
    fn the_tier_is_opt_in_and_refuses_a_bucket_that_forgets_too_soon() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let today = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let old = today - chrono::Duration::days(COLD_AFTER_DAYS + 5);
        let (mut w, _) =
            openlens_storage::Writer::open(root, openlens_storage::FlushPolicy::default()).unwrap();
        w.append(&[event(old, "ann", 1)]).unwrap();
        w.flush().unwrap();
        let g = openlens_storage::rollup::generation();
        assert!(
            openlens_storage::rollup::write(root, "p", old, g, &openlens_storage::rollup::empty())
                .unwrap()
        );
        let (url, store) = fake_store();
        let cfg = cfg(url);
        let meta = openlens_meta::Meta::in_memory().unwrap();
        s3::sync(&cfg, root, &meta, &today.to_string()).unwrap();
        let old_dir = root.join(rel("p", old));

        // Off by default: S3 is a second copy, nothing leaves the disk.
        assert_eq!(offload_checked(&cfg, root, &meta, today).unwrap(), 0);
        assert!(old_dir.is_dir());
        // On, but the bucket expires objects after 30 days: refused, said why.
        meta.set_setting(OPT_IN, "on").unwrap();
        store.lock().unwrap().insert(
            "__lifecycle__".into(),
            b"<LifecycleConfiguration><Rule><Status>Enabled</Status><Expiration><Days>30</Days></Expiration></Rule></LifecycleConfiguration>".to_vec(),
        );
        assert_eq!(offload_checked(&cfg, root, &meta, today).unwrap(), 0);
        assert!(old_dir.is_dir(), "no local file removed");
        assert!(blocked(&meta).unwrap().contains("30 days"));
        // No lifecycle rule: the day moves, and the warning clears.
        store.lock().unwrap().remove("__lifecycle__");
        assert_eq!(offload_checked(&cfg, root, &meta, today).unwrap(), 1);
        assert!(!old_dir.exists());
        assert_eq!(blocked(&meta), None);
    }

    #[test]
    fn old_days_move_to_s3_only_when_verified_and_stay_erasable() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let today = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let old = today - chrono::Duration::days(COLD_AFTER_DAYS + 5);
        let recent = today - chrono::Duration::days(10);
        let (mut w, _) =
            openlens_storage::Writer::open(root, openlens_storage::FlushPolicy::default()).unwrap();
        w.append(&[
            event(old, "ann", 1),
            event(old, "bob", 2),
            event(recent, "ann", 3),
        ])
        .unwrap();
        w.flush().unwrap();
        for day in [old, recent] {
            let tables = openlens_storage::rollup::empty();
            let g = openlens_storage::rollup::generation();
            assert!(openlens_storage::rollup::write(root, "p", day, g, &tables).unwrap());
        }
        let (url, store) = fake_store();
        let cfg = cfg(url);
        let meta = openlens_meta::Meta::in_memory().unwrap();
        let old_dir = root.join(rel("p", old));

        // Not uploaded yet: nothing leaves the disk.
        assert_eq!(offload(&cfg, root, today).unwrap(), 0);
        // Uploaded but S3 lost a file: still kept.
        s3::sync(&cfg, root, &meta, &today.to_string()).unwrap();
        let k = store
            .lock()
            .unwrap()
            .keys()
            .find(|k| k.contains(&format!("date={old}")))
            .cloned()
            .unwrap();
        let saved = store.lock().unwrap().remove(&k).unwrap();
        assert_eq!(offload(&cfg, root, today).unwrap(), 0);
        assert!(old_dir.is_dir());
        store.lock().unwrap().insert(k, saved);
        // Verified: the old day goes, the recent one stays.
        assert_eq!(offload(&cfg, root, today).unwrap(), 1);
        assert!(!old_dir.exists());
        assert!(root.join(rel("p", recent)).is_dir());
        assert_eq!(cold_days(root), [("p".to_owned(), old)]);

        // NSEC-02: losing the local marker (a restore, or the disk failing
        // while the bucket survived) must not stop erasure, expiry or
        // export from reaching this day: S3 is still asked, and the
        // marker is rebuilt from it.
        fs::remove_dir_all(root.join("cold")).unwrap();
        assert!(cold_days(root).is_empty(), "marker lost");
        assert_eq!(
            authoritative_cold_days(&cfg, root).unwrap(),
            [("p".to_owned(), old)]
        );
        assert_eq!(
            cold_days(root),
            [("p".to_owned(), old)],
            "marker rebuilt from S3"
        );

        // Erasing bob reaches the cold day, which goes back to S3 without him.
        let ids: HashSet<String> = ["bob".to_owned()].into();
        assert_eq!(erase(&cfg, root, "p", &ids).unwrap(), 1);
        assert!(!old_dir.exists());
        restore_project(&cfg, root, "p").unwrap();
        assert_eq!(
            openlens_storage::count_rows(root, "p").unwrap(),
            2,
            "ann twice, bob gone"
        );
        assert!(cold_days(root).is_empty());
        // Back on disk and still current: it moves again at once.
        assert_eq!(offload(&cfg, root, today).unwrap(), 1);

        // Past the 400 days, its objects go.
        let later = old + chrono::Duration::days(openlens_storage::EVENT_RETENTION_DAYS + 1);
        assert_eq!(expire(&cfg, root, later).unwrap(), 1);
        assert!(
            !store
                .lock()
                .unwrap()
                .keys()
                .any(|k| k.starts_with(&format!("prod/{}", rel("p", old))))
        );
        assert!(cold_days(root).is_empty());
    }
}
