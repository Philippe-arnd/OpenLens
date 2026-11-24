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

fn rel(project: &str, day: NaiveDate) -> String {
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
            if let Some(parent) = m.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&m, listing(&local)).map_err(|e| e.to_string())?;
            fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            moved += 1;
        }
    }
    Ok(moved)
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
    for (p, day) in cold_days(root) {
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
    for (p, day) in cold_days(root) {
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
    for (p, day) in cold_days(root) {
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
        if let Some(parent) = m.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&m, listing(&files(&dir))).map_err(|e| e.to_string())?;
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

/// Delete the S3 objects of cold days past the event retention (D-015).
/// Returns the days expired.
pub(crate) fn expire(cfg: &Config, root: &Path, today: NaiveDate) -> Result<usize, String> {
    let cutoff = today - chrono::Duration::days(openlens_storage::EVENT_RETENTION_DAYS);
    let mut n = 0;
    for (p, day) in cold_days(root) {
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
                    // A real bucket lists only at its root (`/bucket`).
                    "GET" if query.contains("list-type=2") && !key.is_empty() => {
                        ("404 Not Found", b"<Code>NoSuchKey</Code>".to_vec())
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
