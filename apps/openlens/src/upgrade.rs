//! Safe upgrades (#199): installs may update themselves, so the binary
//! protects the data on its own.
//!
//! `openlens-version.json` in the data directory records the version that
//! last ran and the data format it wrote:
//! - a newer format than this binary knows (a rollback to an older image)
//!   refuses to start, instead of reading or migrating data it does not
//!   understand;
//! - a different version takes a backup first, in `backups/upgrade/` (only
//!   the latest one is kept: it is as large as the data).

use std::fs;
use std::path::Path;

/// Data format written by this binary. Bump it when a change cannot be read
/// by the previous release (a new required column, a new file layout); the
/// additive migrations in `meta` do not need a bump.
pub const FORMAT: u32 = 1;

const FILE: &str = "openlens-version.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct Stamp {
    version: String,
    format: u32,
}

/// What happened, for the log.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A new data directory.
    Fresh,
    /// Same version as last time.
    Same,
    /// Another version ran before; a backup was taken.
    Upgraded { from: String, backup: String },
}

/// Check the data directory before anything opens it. `version` is this
/// binary's version.
pub fn prepare(data: &Path, version: &str) -> Result<Outcome, String> {
    let path = data.join(FILE);
    let previous: Option<Stamp> = match fs::read_to_string(&path) {
        Ok(s) => Some(serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some(p) = &previous
        && p.format > FORMAT
    {
        return Err(format!(
            "this data was written by OpenLens {} (data format {}), and OpenLens {version} only reads format {FORMAT}: run {} or newer",
            p.version, p.format, p.version
        ));
    }
    // Data from before the stamp existed (v0.1.0) counts as another version.
    let had_data = data.join("meta.db").exists();
    let from = match &previous {
        Some(p) if p.version == version => None,
        Some(p) => Some(p.version.clone()),
        None if had_data => Some("0.1.0 or older".to_owned()),
        None => None,
    };
    let outcome = match from {
        None if previous.is_some() => Outcome::Same,
        None => Outcome::Fresh,
        Some(from) => {
            let into = data.join("backups").join("upgrade");
            fs::create_dir_all(&into).map_err(|e| e.to_string())?;
            let (dest, _, _) = crate::backup::rotate(data, &into, 1, chrono::Utc::now())
                .map_err(|e| format!("backup before the upgrade failed, not starting: {e}"))?;
            Outcome::Upgraded {
                from,
                backup: dest.display().to_string(),
            }
        }
    };
    let stamp = Stamp {
        version: version.to_owned(),
        format: FORMAT,
    };
    let tmp = data.join(format!(".{FILE}.tmp"));
    fs::write(&tmp, serde_json::to_vec(&stamp).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn fresh_then_same_then_upgrade_backs_up_and_newer_format_refuses() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path();
        assert_eq!(prepare(data, "0.2.0").unwrap(), Outcome::Fresh);
        openlens_meta::Meta::open(&data.join("meta.db")).unwrap();
        assert_eq!(prepare(data, "0.2.0").unwrap(), Outcome::Same);
        let Outcome::Upgraded { from, backup } = prepare(data, "0.3.0").unwrap() else {
            panic!("expected an upgrade");
        };
        assert_eq!(from, "0.2.0");
        assert!(Path::new(&backup).join("meta.db").exists());
        // Only the latest upgrade backup is kept (copies are named by the
        // second, so wait for the next one).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        prepare(data, "0.4.0").unwrap();
        let n = fs::read_dir(data.join("backups/upgrade")).unwrap().count();
        assert_eq!(n, 1);
        // An older binary on newer data refuses to start.
        fs::write(data.join(FILE), r#"{"version":"9.0.0","format":99}"#).unwrap();
        let err = prepare(data, "0.4.0").unwrap_err();
        assert!(err.contains("9.0.0"), "{err}");
    }

    #[test]
    fn data_from_before_the_stamp_is_backed_up() {
        let d = tempfile::tempdir().unwrap();
        openlens_meta::Meta::open(&d.path().join("meta.db")).unwrap();
        assert!(matches!(
            prepare(d.path(), "0.2.0").unwrap(),
            Outcome::Upgraded { .. }
        ));
    }
}
