//! Code paths per feature (#576, D-092): the `paths` globs of a feature
//! map entry, their validation, and the matcher that says which features a
//! set of changed files touched.
//!
//! The glob syntax is a small subset, matched case-sensitively like git
//! paths: `*` matches any run of characters inside one path segment, `?`
//! one character, and a `**` segment any number of segments (zero
//! included). Everything else is literal. Paths are relative to the
//! repository root, with `/` as separator.
//!
//! Both loops are the classic wildcard matcher (backtrack to the last
//! star only), so one match costs at most `pattern × value` steps, and the
//! whole call is bounded by [`MAX_CHANGED_FILES`] and [`MAX_GLOB_CHECKS`].

use crate::{FeatureMap, MAX_FEATURES};
use serde::Serialize;

/// Maximum code path globs per feature.
pub const MAX_PATHS: usize = 20;
/// Longest code path glob, in bytes.
pub const MAX_PATH_LEN: usize = 200;
/// Changed files examined per call; the rest are counted as skipped.
pub const MAX_CHANGED_FILES: usize = 3000;
/// Longest changed file path examined, in bytes; longer ones are skipped.
pub const MAX_FILE_PATH_LEN: usize = 1024;
/// Glob checks (file × glob) per call; once spent, the remaining files are
/// counted as skipped.
pub const MAX_GLOB_CHECKS: usize = 2_000_000;

/// Is this a code path glob the map accepts? `Err` says why not.
pub fn check_code_path(g: &str) -> Result<(), &'static str> {
    if g.is_empty() || g.len() > MAX_PATH_LEN {
        return Err("a code path is 1-200 characters");
    }
    if g.starts_with('/') || g.as_bytes().get(1) == Some(&b':') {
        return Err(
            "code paths are relative to the repository root: `src/export/**`, not `/src/export/**`",
        );
    }
    if g.chars()
        .any(|c| c.is_control() || matches!(c, '\\' | '[' | ']' | '{' | '}' | '!'))
    {
        return Err("only `*`, `**`, `?` and literal names are supported, with `/` as separator");
    }
    for seg in g.split('/') {
        match seg {
            "" => {
                return Err(
                    "empty segment: no `//` and no trailing `/` (write `src/export/**` for a folder)",
                );
            }
            "." | ".." => {
                return Err("no `.` or `..` segments: write the path from the repository root");
            }
            s if s != "**" && s.contains("**") => {
                return Err("`**` must be a whole segment: `src/**/export.rs`, not `src/a**`");
            }
            _ => {}
        }
    }
    Ok(())
}

/// One feature touched by the changed files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TouchedFeature {
    /// The feature's key.
    pub key: String,
    /// Changed files that match at least one of its paths.
    pub files: usize,
}

/// What [`touched_features`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Touched {
    /// Features with at least one matching file, in map order.
    pub features: Vec<TouchedFeature>,
    /// Examined files that match no feature's paths.
    pub unmatched: usize,
    /// Files not examined: past [`MAX_CHANGED_FILES`], longer than
    /// [`MAX_FILE_PATH_LEN`], or after [`MAX_GLOB_CHECKS`] was spent.
    pub skipped: usize,
    /// Features of the map that declare paths. With none, every file is
    /// unmatched and says nothing about the map's gaps.
    pub features_with_paths: usize,
}

/// A glob split into segments once.
enum Seg<'a> {
    /// `**`: any number of segments.
    Any,
    /// A segment glob (`*` and `?` inside one segment).
    One(&'a [u8]),
}

fn compile(g: &str) -> Option<Vec<Seg<'_>>> {
    check_code_path(g).ok()?;
    Some(
        g.split('/')
            .map(|s| {
                if s == "**" {
                    Seg::Any
                } else {
                    Seg::One(s.as_bytes())
                }
            })
            .collect(),
    )
}

/// `*` and `?` inside one segment. Bytes: `?` matches one byte, so a
/// multibyte character needs as many `?` (paths are mostly ASCII).
fn seg_match(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while si < s.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some(pi);
                mark = si;
                pi += 1;
            }
            Some(&c) if c == b'?' || c == s[si] => {
                pi += 1;
                si += 1;
            }
            _ => match star {
                Some(st) => {
                    pi = st + 1;
                    mark += 1;
                    si = mark;
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&b'*') {
        pi += 1;
    }
    pi == p.len()
}

/// The same matcher one level up: `**` is the star, a segment glob one item.
fn path_match(p: &[Seg<'_>], s: &[&str]) -> bool {
    let (mut pi, mut si, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while si < s.len() {
        match p.get(pi) {
            Some(Seg::Any) => {
                star = Some(pi);
                mark = si;
                pi += 1;
            }
            Some(Seg::One(g)) if seg_match(g, s[si].as_bytes()) => {
                pi += 1;
                si += 1;
            }
            _ => match star {
                Some(st) => {
                    pi = st + 1;
                    mark += 1;
                    si = mark;
                }
                None => return false,
            },
        }
    }
    while matches!(p.get(pi), Some(Seg::Any)) {
        pi += 1;
    }
    pi == p.len()
}

/// Does this repository-relative file path match this code path glob?
/// An invalid glob matches nothing.
pub fn code_path_matches(glob: &str, file: &str) -> bool {
    let Some(p) = compile(glob) else {
        return false;
    };
    let file = file.strip_prefix("./").unwrap_or(file);
    file.len() <= MAX_FILE_PATH_LEN && path_match(&p, &file.split('/').collect::<Vec<_>>())
}

/// Which features the changed files touch, from each feature's `paths`
/// (#576). A file can touch several features. Pure and bounded: at most
/// [`MAX_FEATURES`] features × [`MAX_PATHS`] globs, [`MAX_CHANGED_FILES`]
/// files and [`MAX_GLOB_CHECKS`] checks; invalid globs are ignored.
pub fn touched_features<S: AsRef<str>>(map: &FeatureMap, changed: &[S]) -> Touched {
    let globs: Vec<(usize, Vec<Vec<Seg<'_>>>)> = map
        .features
        .iter()
        .take(MAX_FEATURES)
        .enumerate()
        .map(|(i, f)| {
            (
                i,
                f.paths
                    .iter()
                    .take(MAX_PATHS)
                    .filter_map(|g| compile(g))
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, g)| !g.is_empty())
        .collect();
    let mut counts = vec![0usize; map.features.len()];
    let mut out = Touched {
        features_with_paths: globs.len(),
        skipped: changed.len().saturating_sub(MAX_CHANGED_FILES),
        ..Touched::default()
    };
    let per_file: usize = globs.iter().map(|(_, g)| g.len()).sum();
    let mut checks = 0usize;
    for (n, file) in changed.iter().take(MAX_CHANGED_FILES).enumerate() {
        let file = file.as_ref();
        let file = file.strip_prefix("./").unwrap_or(file);
        if file.is_empty() || file.len() > MAX_FILE_PATH_LEN {
            out.skipped += 1;
            continue;
        }
        if checks.saturating_add(per_file) > MAX_GLOB_CHECKS {
            out.skipped += changed.len().min(MAX_CHANGED_FILES) - n;
            break;
        }
        checks += per_file;
        let segs: Vec<&str> = file.split('/').collect();
        let mut any = false;
        for (i, gs) in &globs {
            if gs.iter().any(|g| path_match(g, &segs)) {
                counts[*i] += 1;
                any = true;
            }
        }
        if !any {
            out.unmatched += 1;
        }
    }
    out.features = map
        .features
        .iter()
        .zip(counts)
        .filter(|(_, c)| *c > 0)
        .map(|(f, files)| TouchedFeature {
            key: f.key.clone(),
            files,
        })
        .collect();
    out
}
