//! Scheduled backups from the dashboard (#218): daily or weekly at an
//! hour (UTC), into a local folder, optionally also to the S3 destination
//! as a `.tar.gz`, keeping the newest N in both. History, *Back up now*,
//! download of the latest copy, and an email to admins on failure (when
//! SMTP is set). `openlens backup` and the Kubernetes CronJob still work.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::AppState;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// The backup settings (all optional: off by default, since a copy takes
/// as much room as the data).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Plan {
    /// `off`, `daily` or `weekly`.
    pub schedule: String,
    /// Hour of day, UTC.
    pub hour: u8,
    /// Day of the week for `weekly`, 1 (Monday) to 7.
    pub weekday: u8,
    /// Copies to keep (local and S3).
    pub keep: usize,
    /// Local folder; empty: `<data dir>/backups`.
    pub dir: String,
    /// Also upload each copy to the S3 destination.
    pub to_s3: bool,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            schedule: "off".into(),
            hour: 3,
            weekday: 1,
            keep: 7,
            dir: String::new(),
            to_s3: false,
        }
    }
}

impl Plan {
    pub(crate) fn load(meta: &openlens_meta::Meta) -> Self {
        meta.setting("backup:plan")
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !["off", "daily", "weekly"].contains(&self.schedule.as_str()) {
            return Err("schedule: off, daily or weekly".into());
        }
        if self.hour > 23 || !(1..=7).contains(&self.weekday) {
            return Err("hour 0-23 and weekday 1-7".into());
        }
        if !(1..=60).contains(&self.keep) {
            return Err("keep 1 to 60 copies".into());
        }
        if !self.dir.is_empty() && !Path::new(&self.dir).is_absolute() {
            return Err("the folder must be an absolute path".into());
        }
        Ok(())
    }

    pub(crate) fn folder(&self, data_dir: &Path) -> PathBuf {
        if self.dir.is_empty() {
            data_dir.join("backups")
        } else {
            PathBuf::from(&self.dir)
        }
    }

    /// Is a scheduled run due at `now`, given the last run?
    pub(crate) fn due(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        last: Option<chrono::DateTime<chrono::Utc>>,
    ) -> bool {
        use chrono::{Datelike, Timelike};
        let gap = match self.schedule.as_str() {
            "daily" => chrono::Duration::hours(20),
            "weekly" => chrono::Duration::days(6),
            _ => return false,
        };
        if now.hour() != u32::from(self.hour) {
            return false;
        }
        if self.schedule == "weekly"
            && now.weekday().number_from_monday() != u32::from(self.weekday)
        {
            return false;
        }
        last.is_none_or(|l| now - l >= gap)
    }
}

/// One run, for the history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Run {
    pub at: String,
    pub ok: bool,
    pub files: usize,
    pub bytes: u64,
    pub secs: f64,
    pub dest: String,
    pub s3: Option<String>,
    pub error: Option<String>,
    /// `schedule` or `manual`.
    pub how: String,
}

pub(crate) fn history(meta: &openlens_meta::Meta) -> Vec<Run> {
    meta.setting("backup:history")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn push_history(meta: &openlens_meta::Meta, run: Run) {
    let mut h = history(meta);
    h.insert(0, run);
    h.truncate(30);
    if let Ok(j) = serde_json::to_string(&h) {
        let _ = meta.set_setting("backup:history", &j);
    }
}

pub(crate) fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(es) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in es.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// Run one backup now (blocking). Refuses to start a second one at once.
pub(crate) fn run(state: &AppState, how: &str) -> Result<Run, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("a backup is already running".into());
    }
    let started = std::time::Instant::now();
    let plan = Plan::load(&state.meta);
    let into = plan.folder(&state.data_dir);
    let res = (|| {
        std::fs::create_dir_all(&into).map_err(|e| format!("{}: {e}", into.display()))?;
        crate::flush_now(state)?;
        let (dest, files, _) =
            crate::backup::rotate(&state.data_dir, &into, plan.keep, chrono::Utc::now())?;
        let bytes = dir_size(&dest);
        let mut s3 = None;
        if plan.to_s3 {
            let Some((cfg, _)) = crate::s3::configured(&state.meta, &state.data_dir) else {
                return Err("copying to S3 is on, but no S3 destination is set".into());
            };
            let name = dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let tmp = into.join(format!(".{name}.tar.gz.tmp"));
            archive(&dest, &tmp)?;
            let body = std::fs::read(&tmp).map_err(|e| e.to_string());
            let _ = std::fs::remove_file(&tmp);
            let key = format!("backups/{name}.tar.gz");
            crate::s3::put(&cfg, &key, &body?)?;
            prune_s3(&cfg, plan.keep)?;
            s3 = Some(key);
        }
        Ok::<_, String>((dest, files, bytes, s3))
    })();
    RUNNING.store(false, Ordering::SeqCst);
    let at = chrono::Utc::now().to_rfc3339();
    let _ = state.meta.set_setting("backup:last", &at);
    let run = match res {
        Ok((dest, files, bytes, s3)) => Run {
            at,
            ok: true,
            files,
            bytes,
            secs: started.elapsed().as_secs_f64(),
            dest: dest.display().to_string(),
            s3,
            error: None,
            how: how.into(),
        },
        Err(e) => Run {
            at,
            ok: false,
            files: 0,
            bytes: 0,
            secs: started.elapsed().as_secs_f64(),
            dest: into.display().to_string(),
            s3: None,
            error: Some(e),
            how: how.into(),
        },
    };
    push_history(&state.meta, run.clone());
    Ok(run)
}

/// Keep the newest `keep` `backups/*.tar.gz` objects on S3.
fn prune_s3(cfg: &crate::s3::Config, keep: usize) -> Result<(), String> {
    let mut keys: Vec<String> = crate::s3::list(cfg, "backups/")?
        .into_iter()
        .filter(|k| k.ends_with(".tar.gz"))
        .collect();
    keys.sort();
    let extra = keys.len().saturating_sub(keep);
    for k in keys.iter().take(extra) {
        crate::s3::delete(cfg, k)?;
    }
    Ok(())
}

/// The newest local copy, if any.
pub(crate) fn latest(state: &AppState) -> Option<PathBuf> {
    let into = Plan::load(&state.meta).folder(&state.data_dir);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(into)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    chrono::NaiveDateTime::parse_from_str(n, "%Y-%m-%dT%H-%M-%SZ").is_ok()
                })
        })
        .collect();
    dirs.sort();
    dirs.pop()
}

/// Write `dir` as a `.tar.gz` at `out` (ustar; names under the copy's own
/// folder name).
pub(crate) fn archive(dir: &Path, out: &Path) -> Result<(), String> {
    let root = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "backup".into());
    let f = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    let mut stack = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    for p in files {
        let rel = p.strip_prefix(dir).map_err(|e| e.to_string())?;
        let name = format!("{root}/{}", rel.to_string_lossy().replace('\\', "/"));
        let data = std::fs::read(&p).map_err(|e| e.to_string())?;
        gz.write_all(&tar_header(&name, data.len() as u64)?)
            .map_err(|e| e.to_string())?;
        gz.write_all(&data).map_err(|e| e.to_string())?;
        let pad = (512 - data.len() % 512) % 512;
        gz.write_all(&vec![0u8; pad]).map_err(|e| e.to_string())?;
    }
    gz.write_all(&[0u8; 1024]).map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn tar_header(name: &str, size: u64) -> Result<[u8; 512], String> {
    let mut h = [0u8; 512];
    let (prefix, base) = if name.len() <= 100 {
        ("", name)
    } else {
        let cut = name[..name.len().min(156)]
            .rfind('/')
            .filter(|&i| name.len() - i - 1 <= 100)
            .ok_or_else(|| format!("path too long for the archive: {name}"))?;
        (&name[..cut], &name[cut + 1..])
    };
    let put = |h: &mut [u8; 512], at: usize, s: &[u8]| h[at..at + s.len()].copy_from_slice(s);
    put(&mut h, 0, base.as_bytes());
    put(&mut h, 100, b"0000644\0");
    put(&mut h, 108, b"0000000\0");
    put(&mut h, 116, b"0000000\0");
    put(&mut h, 124, format!("{size:011o}\0").as_bytes());
    let mtime = chrono::Utc::now().timestamp().max(0);
    put(&mut h, 136, format!("{mtime:011o}\0").as_bytes());
    put(&mut h, 148, b"        ");
    h[156] = b'0';
    put(&mut h, 257, b"ustar\0");
    put(&mut h, 263, b"00");
    put(&mut h, 345, prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    put(&mut h, 148, format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

/// Called every minute by the background loop: run a due scheduled backup
/// in the background, and email admins when it fails.
pub(crate) async fn tick(state: &AppState) {
    let plan = Plan::load(&state.meta);
    let last = state
        .meta
        .setting("backup:last")
        .ok()
        .flatten()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.to_utc());
    if running() || !plan.due(chrono::Utc::now(), last) {
        return;
    }
    let st = state.clone();
    let Ok(Ok(r)) = tokio::task::spawn_blocking(move || run(&st, "schedule")).await else {
        return;
    };
    if r.ok {
        tracing::info!(files = r.files, bytes = r.bytes, "scheduled backup done");
        return;
    }
    tracing::warn!(error = ?r.error, "scheduled backup failed");
    if let Some((url, from)) = crate::digest::login_mail() {
        let admins: Vec<String> = state
            .meta
            .users()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| u.role == "admin" && !u.disabled)
            .map(|u| u.email)
            .collect();
        if !admins.is_empty() {
            let body = format!(
                "The scheduled OpenLens backup failed at {}:\n\n{}\n\nSee Settings → Storage and backups.",
                r.at,
                r.error.unwrap_or_default()
            );
            let _ =
                crate::digest::send_mail(&url, &from, &admins, "OpenLens: backup failed", &body)
                    .await;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_plan_is_due_once_at_its_hour() {
        let p = Plan {
            schedule: "daily".into(),
            hour: 3,
            ..Plan::default()
        };
        let at = |d, h, m| chrono::Utc.with_ymd_and_hms(2026, 11, d, h, m, 0).unwrap();
        assert!(p.due(at(23, 3, 0), None));
        assert!(!p.due(at(23, 4, 0), None));
        assert!(!p.due(at(23, 3, 30), Some(at(23, 3, 0))));
        assert!(p.due(at(24, 3, 0), Some(at(23, 3, 0))));
        let w = Plan {
            schedule: "weekly".into(),
            weekday: 1,
            ..p.clone()
        };
        assert!(w.due(at(23, 3, 0), None), "2026-11-23 is a Monday");
        assert!(!w.due(at(24, 3, 0), None));
        assert!(!Plan::default().due(at(23, 3, 0), None));
        assert!(
            Plan {
                keep: 0,
                ..Plan::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Plan {
                dir: "rel".into(),
                ..Plan::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn archives_a_copy_as_tar_gz() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("2026-11-23T03-00-00Z");
        std::fs::create_dir_all(src.join("events/project=p")).unwrap();
        std::fs::write(src.join("events/project=p/a.parquet"), b"hello").unwrap();
        std::fs::write(src.join("meta.db"), b"db").unwrap();
        let out = d.path().join("x.tar.gz");
        archive(&src, &out).unwrap();
        let mut raw = Vec::new();
        std::io::Read::read_to_end(
            &mut flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap()),
            &mut raw,
        )
        .unwrap();
        assert_eq!(raw.len() % 512, 0);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("2026-11-23T03-00-00Z/events/project=p/a.parquet"));
        assert!(text.contains("hello") && text.contains("ustar"));
        let sum: u32 = raw[..512]
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u32::from(*b)
                }
            })
            .sum();
        let stored =
            u32::from_str_radix(String::from_utf8_lossy(&raw[148..154]).trim(), 8).unwrap();
        assert_eq!(sum, stored, "checksum");
    }
}
