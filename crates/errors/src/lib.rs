//! Source-map resolution: turn a minified stack frame back into the
//! original file, line and function.
//!
//! Layer: L2 (see `docs/architecture.md`). Implements the public Source Map
//! revision 3 format (base64 VLQ `mappings`), with no dependency. Maps are
//! stored per project and release:
//!
//! ```text
//! <data>/sourcemaps/project=<p>/release=<r>/<bundle file name>.map
//! ```
//!
//! Every input is bounded: maps up to [`MAX_MAP_BYTES`], ids and file names
//! restricted to a path-safe alphabet.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Largest accepted source map, in bytes.
pub const MAX_MAP_BYTES: usize = 32 * 1024 * 1024;

/// Errors from source-map handling.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An id or file name outside the allowed alphabet.
    #[error("invalid name")]
    InvalidName,
    /// Not a version 3 source map, or a malformed one.
    #[error("not a valid source map: {0}")]
    Invalid(&'static str),
    /// Too large.
    #[error("source map is too large")]
    TooLarge,
    /// Disk failure.
    #[error("source map storage: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// An original position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Original {
    /// Original source file.
    pub source: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    /// Original name at that position, when the map has one.
    pub name: Option<String>,
}

#[derive(Deserialize)]
struct Raw {
    version: u32,
    #[serde(default)]
    sources: Vec<Option<String>>,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    mappings: String,
    #[serde(default, rename = "sourceRoot")]
    source_root: Option<String>,
}

/// One mapped segment: generated column → original position.
#[derive(Debug, Clone, Copy)]
struct Seg {
    col: u32,
    src: u32,
    line: u32,
    ocol: u32,
    name: Option<u32>,
}

/// A parsed source map.
#[derive(Debug)]
pub struct SourceMap {
    sources: Vec<String>,
    names: Vec<String>,
    /// Segments per generated line, sorted by column.
    lines: Vec<Vec<Seg>>,
}

fn b64(c: u8) -> Option<i64> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as i64)
}

/// Decode the base64 VLQ numbers of one segment.
fn vlq(seg: &[u8], out: &mut Vec<i64>) -> Option<()> {
    out.clear();
    let (mut value, mut shift) = (0i64, 0u32);
    for &c in seg {
        let d = b64(c)?;
        value += (d & 31) << shift;
        if d & 32 != 0 {
            shift += 5;
            if shift > 30 {
                return None;
            }
        } else {
            let neg = value & 1 == 1;
            value >>= 1;
            out.push(if neg { -value } else { value });
            value = 0;
            shift = 0;
        }
    }
    (shift == 0).then_some(())
}

impl SourceMap {
    /// Parse a source map (JSON text).
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > MAX_MAP_BYTES {
            return Err(Error::TooLarge);
        }
        let raw: Raw = serde_json::from_str(text).map_err(|_| Error::Invalid("not JSON"))?;
        if raw.version != 3 {
            return Err(Error::Invalid("version must be 3"));
        }
        let root = raw.source_root.unwrap_or_default();
        let sources = raw
            .sources
            .into_iter()
            .map(|s| {
                let s = s.unwrap_or_default();
                if root.is_empty() || s.contains("://") {
                    s
                } else {
                    format!("{}/{s}", root.trim_end_matches('/'))
                }
            })
            .collect::<Vec<_>>();
        let (mut src, mut line, mut ocol, mut name) = (0i64, 0i64, 0i64, 0i64);
        let mut lines = Vec::new();
        let mut nums = Vec::with_capacity(5);
        for gen_line in raw.mappings.split(';') {
            let mut col = 0i64;
            let mut segs = Vec::new();
            for seg in gen_line.split(',').filter(|s| !s.is_empty()) {
                vlq(seg.as_bytes(), &mut nums).ok_or(Error::Invalid("bad mappings"))?;
                col += nums[0];
                if col < 0 {
                    return Err(Error::Invalid("negative column"));
                }
                if nums.len() < 4 {
                    continue; // a segment with no original position
                }
                src += nums[1];
                line += nums[2];
                ocol += nums[3];
                let n = if nums.len() >= 5 {
                    name += nums[4];
                    Some(name)
                } else {
                    None
                };
                let ok = |v: i64| u32::try_from(v).ok();
                segs.push(Seg {
                    col: ok(col).ok_or(Error::Invalid("negative column"))?,
                    src: ok(src).ok_or(Error::Invalid("bad source index"))?,
                    line: ok(line).ok_or(Error::Invalid("negative line"))?,
                    ocol: ok(ocol).ok_or(Error::Invalid("negative column"))?,
                    name: n.and_then(ok),
                });
            }
            segs.sort_by_key(|s| s.col);
            lines.push(segs);
        }
        Ok(Self {
            sources,
            names: raw.names,
            lines,
        })
    }

    /// The original position of a generated 1-based `line` and `column`
    /// (as browsers print them in stack traces).
    pub fn lookup(&self, line: u32, column: u32) -> Option<Original> {
        let segs = self.lines.get(line.checked_sub(1)? as usize)?;
        let col = column.saturating_sub(1);
        let i = segs.partition_point(|s| s.col <= col);
        let s = segs.get(i.checked_sub(1)?)?;
        Some(Original {
            source: self.sources.get(s.src as usize)?.clone(),
            line: s.line + 1,
            column: s.ocol + 1,
            name: s.name.and_then(|n| self.names.get(n as usize).cloned()),
        })
    }
}

/// Is `s` usable as a path component (project, release or bundle name)?
pub fn valid_name(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@+".contains(&b))
}

fn map_path(root: &Path, project: &str, release: &str, bundle: &str) -> Result<PathBuf> {
    if ![project, release, bundle].iter().all(|s| valid_name(s)) {
        return Err(Error::InvalidName);
    }
    Ok(root
        .join("sourcemaps")
        .join(format!("project={project}"))
        .join(format!("release={release}"))
        .join(format!("{bundle}.map")))
}

/// Validate and store the map of `bundle` (the minified file's name, e.g.
/// `app.3f9a.js`) for one release. Re-uploading replaces it.
pub fn store(root: &Path, project: &str, release: &str, bundle: &str, text: &str) -> Result<()> {
    SourceMap::parse(text)?;
    let path = map_path(root, project, release, bundle)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, path)?;
    Ok(())
}

/// Load the stored map of `bundle` for a release, if any.
pub fn load(root: &Path, project: &str, release: &str, bundle: &str) -> Result<Option<SourceMap>> {
    let path = map_path(root, project, release, bundle)?;
    match fs::read_to_string(path) {
        Ok(t) => SourceMap::parse(&t).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// One frame of a browser stack trace: the bundle's file name, line and
/// column, e.g. from `at f (https://x/assets/app.3f9a.js:1:4567)` or
/// `f@https://x/assets/app.3f9a.js:1:4567`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Function name as printed, if any.
    pub function: Option<String>,
    /// Last path segment of the script URL.
    pub bundle: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
}

/// Parse the frames of a V8 (`at …`) or Firefox/Safari (`fn@url`) stack.
pub fn frames(stack: &str) -> Vec<Frame> {
    stack
        .lines()
        .take(50)
        .filter_map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("at ").unwrap_or(l);
            let (function, loc) = if let Some((f, rest)) = l.rsplit_once(" (") {
                (Some(f.trim()), rest.trim_end_matches(')'))
            } else if let Some((f, rest)) = l.split_once('@') {
                ((!f.is_empty()).then_some(f), rest)
            } else {
                (None, l)
            };
            let mut parts = loc.rsplitn(3, ':');
            let column = parts.next()?.parse().ok()?;
            let line = parts.next()?.parse().ok()?;
            let url = parts.next()?;
            let bundle = url.split(['?', '#']).next()?.rsplit('/').next()?.to_owned();
            valid_name(&bundle).then(|| Frame {
                function: function.map(str::to_owned),
                bundle,
                line,
                column,
            })
        })
        .collect()
}

/// Resolve the first frame of `stack` that has a stored map for `release`.
pub fn resolve(root: &Path, project: &str, release: &str, stack: &str) -> Option<Original> {
    frames(stack).iter().find_map(|f| {
        load(root, project, release, &f.bundle)
            .ok()
            .flatten()
            .and_then(|m| m.lookup(f.line, f.column))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    // Hand-checked map: generated line 1 has segments at col 0 → a.ts 1:1,
    // col 10 → a.ts 3:5 name "boom", and line 2 col 4 → b.ts 10:1.
    // "AAAA"  = [0,0,0,0]     col 0, source 0, line 0, col 0
    // "UAEIA" = [10,0,2,4,0]  col 10, line +2, col +4, name 0
    // "ICOJ"  = [4,1,7,-4]    next line: col 4, source +1, line +7 (= 9), col 0
    const MAP: &str =
        r#"{"version":3,"sources":["a.ts","b.ts"],"names":["boom"],"mappings":"AAAA,UAEIA;ICOJ"}"#;

    #[test]
    fn decodes_vlq_and_looks_up_positions() {
        let m = SourceMap::parse(MAP).unwrap();
        let o = m.lookup(1, 1).unwrap();
        assert_eq!((o.source.as_str(), o.line, o.column), ("a.ts", 1, 1));
        let o = m.lookup(1, 15).unwrap();
        assert_eq!(
            (o.source.as_str(), o.line, o.column, o.name.as_deref()),
            ("a.ts", 3, 5, Some("boom"))
        );
        let o = m.lookup(2, 5).unwrap();
        assert_eq!((o.source.as_str(), o.line), ("b.ts", 10));
        assert!(m.lookup(2, 1).is_none(), "before the first segment");
        assert!(m.lookup(9, 1).is_none());
    }

    #[test]
    fn parses_v8_and_firefox_frames() {
        let v8 = "TypeError: x is undefined\n    at pay (https://app.example.com/assets/app.3f9a.js:1:4567)\n    at https://app.example.com/assets/vendor.js?v=2:3:10";
        let f = frames(v8);
        assert_eq!(f.len(), 2);
        assert_eq!(
            (
                f[0].function.as_deref(),
                f[0].bundle.as_str(),
                f[0].line,
                f[0].column
            ),
            (Some("pay"), "app.3f9a.js", 1, 4567)
        );
        assert_eq!((f[1].bundle.as_str(), f[1].line), ("vendor.js", 3));
        let ff =
            frames("pay@https://app.example.com/assets/app.3f9a.js:1:4567\n@https://x/y.js:2:3");
        assert_eq!(ff[0].function.as_deref(), Some("pay"));
        assert_eq!(ff[1].bundle, "y.js");
    }

    #[test]
    fn store_validates_and_resolve_uses_the_release() {
        let d = tempfile::tempdir().unwrap();
        assert!(store(d.path(), "p", "v1", "app.js", "{}").is_err());
        assert!(store(d.path(), "p", "../x", "app.js", MAP).is_err());
        assert!(store(d.path(), "p", "v1", "..", MAP).is_err());
        store(d.path(), "p", "v1", "app.js", MAP).unwrap();
        let stack = "Error: boom\n    at f (https://x/app.js:1:12)";
        let o = resolve(d.path(), "p", "v1", stack).unwrap();
        assert_eq!(
            (o.source.as_str(), o.line, o.name.as_deref()),
            ("a.ts", 3, Some("boom"))
        );
        assert!(
            resolve(d.path(), "p", "v2", stack).is_none(),
            "maps are per release"
        );
    }

    #[test]
    fn hostile_maps_are_rejected_not_panicking() {
        for bad in [
            r#"{"version":2,"mappings":""}"#,
            r#"{"version":3,"mappings":"!!!"}"#,
            r#"{"version":3,"mappings":"ggggggggggggggggA"}"#,
            r#"{"version":3,"sources":[],"mappings":"D"}"#,
        ] {
            assert!(SourceMap::parse(bad).is_err(), "{bad}");
        }
        // An index out of range parses but never resolves.
        let m = SourceMap::parse(r#"{"version":3,"sources":[],"mappings":"AAAA"}"#).unwrap();
        assert!(m.lookup(1, 1).is_none());
    }
}
