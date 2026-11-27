//! The features each release touched (#576, part 7 of #237, D-095): the
//! changed files the git host gave for its range (#574,
//! [`crate::release_git`]) matched against the code paths (`paths`) of
//! the feature map in force for the release
//! ([`openlens_featuremap::touched_features`], D-092).
//!
//! - **Once per release:** matched by the server's hourly pass, right
//!   after the git host is read ([`attribute`]), and kept in the metadata
//!   store (`openlens_meta::release_features`); never on a page request.
//!   A release recorded again with another range is matched again.
//! - **The map in force:** the version recorded for the release when its
//!   numbers were first counted (D-070), else the latest version saved
//!   before its first deploy (else the oldest), as [`crate::inforce`]
//!   counts a release's numbers. A later push does not change it.
//! - **Bounds:** the read keeps [`crate::release_git::MAX_FILES`] files,
//!   the matcher its own bounds (files × globs), and [`MAX_TOUCHED`]
//!   features are kept per release, with the total.
//! - **Use:** the verdict judges and lists the touched features first
//!   ([`crate::verdict::compare`]); the release notes carry them
//!   ([`crate::release_git::ReleaseNotes::touched`]), so `releases.list`,
//!   `releases.report` (MCP `release_report`), `releases.notes` and the
//!   Releases page show them; a feature's page lists the releases that
//!   touched it ([`crate::maphealth::FeatureContext::touched_by`]).

use chrono::Utc;
use openlens_featuremap::FeatureMap;
use openlens_meta::Meta;
use openlens_meta::release_features::ReleaseFeatures;
use openlens_meta::release_reads::ReleaseRange;
use serde::Serialize;

use crate::inforce;

/// Touched features kept per release.
pub const MAX_TOUCHED: usize = 100;
/// Releases matched per project on each pass.
pub const PER_PASS: i64 = 20;

/// Where a release's touched features stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchedState {
    /// Matched: `features` are the features touched (maybe none).
    Ready,
    /// The feature map in force for the release declares no code paths.
    NoPaths,
    /// No feature map was saved when the release came out.
    NoMap,
    /// Read from the git host, not matched yet: the next hourly pass does.
    Pending,
}

/// One feature a release touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TouchedFeature {
    /// The feature's key (in the map version matched with).
    pub key: String,
    /// Changed files that match its code paths.
    pub files: i64,
}

/// The features a release touched, or why they are not known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseTouched {
    /// Where they stand.
    pub state: TouchedState,
    /// Why they are not known (every state but `ready`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The map version matched with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_version: Option<i64>,
    /// The features touched, in map order (at most [`MAX_TOUCHED`]).
    pub features: Vec<TouchedFeature>,
    /// Of the features touched, the ones not kept.
    pub features_more: i64,
    /// Changed files matched against the map.
    pub files: i64,
    /// Of those, files that match no feature's code paths.
    pub unmatched: i64,
    /// Changed files not matched: past the read's or the matcher's bounds.
    pub skipped: i64,
    /// The gaps, in plain words: "N changed files match no feature", or
    /// the files left out. Absent when every file matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ReleaseTouched {
    fn none(state: TouchedState, map_version: Option<i64>, reason: String) -> Self {
        Self {
            state,
            reason: Some(reason),
            map_version,
            features: Vec::new(),
            features_more: 0,
            files: 0,
            unmatched: 0,
            skipped: 0,
            note: None,
        }
    }
}

/// The map in force for a release of `environment`: the version recorded
/// for it (D-070), else the latest saved before its first deploy, else
/// the oldest. `None` without a stored map.
fn map_for(
    meta: &Meta,
    project: &str,
    environment: &str,
    release: &str,
) -> Option<(FeatureMap, i64)> {
    let recorded = meta
        .release_maps(project, environment)
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.release == release)
        .map(|r| r.map_version);
    let history = inforce::history(meta, project);
    if let Some(v) = recorded {
        if let Some(m) = history.iter().find(|m| m.version == v) {
            return Some((m.map.clone(), v));
        }
        let yaml = meta.feature_map_version(project, v).ok().flatten();
        if let Some(m) = yaml.and_then(|y| openlens_featuremap::parse(&y).map) {
            return Some((m, v));
        }
    }
    let at = meta
        .release_first_deploy(project, environment, release)
        .ok()
        .flatten()
        .unwrap_or_else(|| Utc::now().timestamp());
    inforce::in_force(&history, at as f64).map(|(m, _)| (m.map.clone(), m.version))
}

/// Match one read release's changed files against `map` (pure): what is
/// stored for it.
#[must_use]
pub fn matched(
    p: &ReleaseRange,
    map: Option<(&FeatureMap, i64)>,
    files: &[String],
    now: i64,
) -> ReleaseFeatures {
    let mut out = ReleaseFeatures {
        environment: p.environment.clone(),
        release: p.release.clone(),
        from: p.from.clone(),
        to: p.to.clone(),
        files: files.len() as i64,
        computed_at: now,
        ..ReleaseFeatures::default()
    };
    let Some((map, version)) = map else {
        return out;
    };
    let t = openlens_featuremap::touched_features(map, files);
    out.map_version = Some(version);
    out.features_with_paths = t.features_with_paths as i64;
    out.files = (files.len() - t.skipped.min(files.len())) as i64;
    out.unmatched = t.unmatched as i64;
    out.skipped = t.skipped as i64;
    out.features_total = t.features.len() as i64;
    out.features = t
        .features
        .into_iter()
        .take(MAX_TOUCHED)
        .map(|f| (f.key, f.files as i64))
        .collect();
    out
}

/// The hourly pass for one project (called by [`crate::release_git::pass`]):
/// match up to [`PER_PASS`] releases read from the git host and not
/// matched yet. Gives the failures, for the caller to log.
pub fn attribute(meta: &Meta, project: &str) -> Vec<String> {
    let Ok(pending) = meta.release_features_pending(project, PER_PASS) else {
        return vec!["could not list the releases to match to features".into()];
    };
    let mut failures = Vec::new();
    for p in pending {
        let files = match meta.release_read_files(project, &p.environment, &p.release) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("{} ({}): {e}", p.release, p.environment));
                continue;
            }
        };
        let map = map_for(meta, project, &p.environment, &p.release);
        let a = matched(
            &p,
            map.as_ref().map(|(m, v)| (m, *v)),
            &files,
            Utc::now().timestamp(),
        );
        if let Err(e) = meta.store_release_features(project, &a) {
            failures.push(format!("{} ({}): {e}", p.release, p.environment));
        }
    }
    failures
}

/// The features `release` of `environment` touched, read for the range
/// `from..to`; `not_read` changed files were listed by the git host but
/// not kept. Reads the store only.
pub fn touched(
    meta: &Meta,
    project: &str,
    environment: &str,
    release: &str,
    (from, to): (&str, &str),
    not_read: i64,
) -> ReleaseTouched {
    let kept = match meta.release_features(project, environment, release) {
        Ok(k) => k.filter(|a| a.from == from && a.to == to),
        Err(e) => {
            return ReleaseTouched::none(
                TouchedState::Pending,
                None,
                format!("Could not read the features touched: {e}"),
            );
        }
    };
    let Some(a) = kept else {
        return ReleaseTouched::none(
            TouchedState::Pending,
            None,
            "Changed files not matched to the feature map yet: the hourly pass does it.".into(),
        );
    };
    let Some(version) = a.map_version else {
        return ReleaseTouched::none(
            TouchedState::NoMap,
            None,
            "No feature map was saved when this release came out, so its changed files are not matched to features.".into(),
        );
    };
    if a.features_with_paths == 0 {
        return ReleaseTouched::none(
            TouchedState::NoPaths,
            Some(version),
            format!(
                "The feature map in force for this release (v{version}) declares no code paths yet, so its {} changed files are not matched to features: run `openlens-cli rules` and have the app's agents add `paths` to each feature.",
                a.files
            ),
        );
    }
    let skipped = a.skipped + not_read.max(0);
    let mut gaps = Vec::new();
    if a.unmatched > 0 {
        gaps.push(format!(
            "{} changed file{} match no feature: add {} folder to a feature's `paths`",
            a.unmatched,
            if a.unmatched == 1 { "" } else { "s" },
            if a.unmatched == 1 { "its" } else { "their" }
        ));
    }
    if skipped > 0 {
        gaps.push(format!(
            "{skipped} changed file{} not matched (past the bounds of one release)",
            if skipped == 1 { "" } else { "s" }
        ));
    }
    ReleaseTouched {
        state: TouchedState::Ready,
        reason: None,
        map_version: Some(version),
        features: a
            .features
            .into_iter()
            .map(|(key, files)| TouchedFeature { key, files })
            .collect(),
        features_more: (a.features_total - MAX_TOUCHED as i64).max(0),
        files: a.files,
        unmatched: a.unmatched,
        skipped,
        note: (!gaps.is_empty()).then(|| format!("{}.", gaps.join("; "))),
    }
}

/// The features each release of `environment` touched, for the matches
/// of the range read now (bounded), newest match first: what the verdict
/// compares first and a feature page lists.
#[must_use]
pub fn by_release(
    meta: &Meta,
    project: &str,
    environment: &str,
) -> Vec<(String, Vec<TouchedFeature>)> {
    let mut out: Vec<(String, Vec<TouchedFeature>)> = Vec::new();
    for (release, key, files) in meta
        .release_features_of(project, environment)
        .unwrap_or_default()
    {
        let f = TouchedFeature { key, files };
        match out.last_mut() {
            Some((r, fs)) if *r == release => fs.push(f),
            _ => out.push((release, vec![f])),
        }
    }
    out
}

/// The releases that touched `key` (#576), under it or a former key.
pub(crate) fn touched_by(
    history: &crate::inforce::History,
    key: &str,
    def: Option<&openlens_featuremap::Feature>,
) -> Vec<(String, i64)> {
    let names: Vec<&str> = std::iter::once(key)
        .chain(
            def.into_iter()
                .flat_map(|d| d.renamed_from.iter().map(String::as_str)),
        )
        .collect();
    history
        .touched
        .iter()
        .filter_map(|(release, fs)| {
            let f = fs.iter().find(|f| names.contains(&f.key.as_str()))?;
            Some((release.clone(), f.files))
        })
        .collect()
}
