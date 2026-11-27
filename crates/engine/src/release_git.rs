//! The git host link, read side (#574, part of #237, D-092): for each
//! release recorded with its commit range (`from..commit`, D-066), the
//! commits of the range, the merged pull requests they belong to, the
//! files they changed and the tag or GitHub release named like it, read
//! from GitHub with read-only calls and kept in the metadata store
//! (`openlens_meta::release_reads`). Release notes are written from the
//! pull request titles, grouped plainly.
//!
//! - **When:** only from the server's hourly pass ([`pass`]), a few
//!   releases per project at a time; never on a page request, never in the
//!   deploy hook or `releases.record`.
//! - **Calls:** the compare API for the range (up to 3 pages), the
//!   repository's recently closed pull requests matched on their merge
//!   commit (up to 3 pages), and the release or tag named like the release.
//! - **Bounds:** [`MAX_COMMITS`], [`MAX_PRS`] and [`MAX_FILES`] kept per
//!   release, with the totals for "and K more"; every string GitHub sends
//!   is cut and checked.
//! - **Failures:** a refusal, an error or a rate limit is returned to the
//!   caller (who logs it) and noted on the release, which is tried again
//!   on the next pass, at most [`MAX_ATTEMPTS`] times.
//! - **Privacy:** no author is kept, not their name, login or email.
//!
//! The GitHub link is the one tickets use (#245): the repository and the
//! sealed token of [`Host::github`].

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use openlens_meta::release_reads::{GitCommit, GitPr, GitRead, ReleaseRange};
use serde::Serialize;
use serde_json::Value;

use crate::commands::Host;
use crate::commands::tickets::GitHub;

/// Commits kept per release.
pub const MAX_COMMITS: usize = 250;
/// Merged pull requests kept per release.
pub const MAX_PRS: usize = 100;
/// Changed file paths kept per release.
pub const MAX_FILES: usize = 300;
/// Failed reads of one range before the pass gives up on it (until the
/// release is recorded again).
pub const MAX_ATTEMPTS: i64 = 5;
/// Releases read per project on each pass.
pub const READS_PER_PASS: i64 = 3;
/// Pages read from one paginated GitHub list (100 items each).
const MAX_PAGES: u32 = 3;
/// Longest pull request title kept, in characters.
const MAX_TITLE: usize = 200;
/// Longest file path kept, in bytes.
const MAX_PATH: usize = 400;
/// Longest URL kept, in bytes.
const MAX_URL: usize = 300;
/// Longest tag kept, in bytes.
const MAX_TAG: usize = 200;
/// Pull requests listed (shown) per release in the notes.
const SHOWN_PRS: usize = MAX_PRS;
/// Releases with notes listed by `releases.list`, newest first.
pub const LISTED: usize = 20;

/// Where a release's notes stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotesState {
    /// Read: the notes are there.
    Ready,
    /// The project has no GitHub repository and token.
    NoLink,
    /// The release was recorded without a commit range.
    NoRange,
    /// Not read yet: the next hourly pass reads it.
    Pending,
    /// The reads failed; see the reason.
    Failed,
}

/// One pull request in the notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoteItem {
    /// Its number.
    pub number: i64,
    /// Its title, without a `feat:`-style prefix.
    pub title: String,
    /// Its page.
    pub url: String,
    /// When it was merged (RFC 3339, UTC).
    pub merged_at: Option<String>,
}

/// A group of the notes: `Added`, `Fixed` or `Changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoteGroup {
    /// The group's title.
    pub title: &'static str,
    /// Its pull requests, in merge order.
    pub items: Vec<NoteItem>,
}

/// A release's notes from the git host, or why there are none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseNotes {
    /// Release name.
    pub release: String,
    /// Environment.
    pub environment: String,
    /// Where they stand.
    pub state: NotesState,
    /// Why they are not there (every state but `ready`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The commit range, `from..to` (short ids), when one was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    /// The tag or GitHub release named like the release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The GitHub release's page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_url: Option<String>,
    /// The merged pull requests, grouped.
    pub groups: Vec<NoteGroup>,
    /// Merged pull requests found.
    pub prs: i64,
    /// Of those, the ones not kept ("and K more").
    pub prs_more: i64,
    /// Commits in the range.
    pub commits: i64,
    /// Of those, the ones not kept.
    pub commits_more: i64,
    /// Changed files listed.
    pub files: i64,
    /// Of those, the ones not kept.
    pub files_more: i64,
    /// The oldest author time of the range's commits (RFC 3339, UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_commit_at: Option<String>,
    /// When it was read (RFC 3339, UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
}

impl ReleaseNotes {
    fn empty(environment: &str, release: &str, state: NotesState, reason: String) -> Self {
        Self {
            release: release.to_owned(),
            environment: environment.to_owned(),
            state,
            reason: Some(reason),
            range: None,
            tag: None,
            tag_url: None,
            groups: Vec::new(),
            prs: 0,
            prs_more: 0,
            commits: 0,
            commits_more: 0,
            files: 0,
            files_more: 0,
            first_commit_at: None,
            fetched_at: None,
        }
    }
}

fn rfc3339(t: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(t, 0).map(|d| d.to_rfc3339())
}

fn parse_time(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    if s.len() > 40 {
        return None;
    }
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

/// A string from GitHub: printable, at most `max` characters, or nothing.
fn clean(v: &Value, max: usize) -> Option<String> {
    let s: String = v
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    let s = s.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// A URL from GitHub: `https://`, short, no spaces or control characters.
fn url(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    (s.starts_with("https://")
        && s.len() <= MAX_URL
        && !s.chars().any(|c| c.is_control() || c.is_whitespace()))
    .then(|| s.to_owned())
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// One page of the compare API, bounded: its commits (valid ids only,
/// oldest first, author time only), the range's total, and on the first
/// page its changed files with how many GitHub listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComparePage {
    /// Commits of the page.
    pub commits: Vec<GitCommit>,
    /// Committer time of the oldest commit of the page (Unix seconds).
    pub oldest_committed: Option<i64>,
    /// Commits in the range, as GitHub counts them.
    pub total: i64,
    /// Changed file paths (at most [`MAX_FILES`]).
    pub files: Vec<String>,
    /// Changed files GitHub listed.
    pub files_listed: i64,
}

/// Read one page of `GET /repos/{repo}/compare/{from}...{to}` (pure).
#[must_use]
pub fn compare_page(v: &Value) -> ComparePage {
    let mut out = ComparePage {
        total: v["total_commits"].as_i64().unwrap_or(0).clamp(0, 1_000_000),
        ..ComparePage::default()
    };
    if let Some(cs) = v["commits"].as_array() {
        for c in cs.iter().take(MAX_COMMITS) {
            let Some(sha) = c["sha"].as_str().and_then(crate::deploys::commit) else {
                continue;
            };
            let at = parse_time(&c["commit"]["author"]["date"]);
            let committed = parse_time(&c["commit"]["committer"]["date"]).or(at);
            if let Some(t) = committed {
                out.oldest_committed = Some(out.oldest_committed.map_or(t, |o| o.min(t)));
            }
            out.commits.push(GitCommit { sha, at });
        }
    }
    if let Some(fs) = v["files"].as_array() {
        out.files_listed = fs.len() as i64;
        let mut seen = HashSet::new();
        for f in fs {
            if out.files.len() >= MAX_FILES {
                break;
            }
            let Some(p) = f["filename"].as_str() else {
                continue;
            };
            if p.is_empty() || p.len() > MAX_PATH || p.chars().any(char::is_control) {
                continue;
            }
            if seen.insert(p) {
                out.files.push(p.to_owned());
            }
        }
    }
    out
}

/// The merged pull requests of one page of `GET /repos/{repo}/pulls`
/// whose merge commit is one of `shas` (pure): squash, merge and rebase
/// merges all leave their merge commit in the range. Also the oldest
/// `updated_at` of the page, to know when to stop paging.
#[must_use]
pub fn merged_in(v: &Value, shas: &HashSet<String>) -> (Vec<GitPr>, Option<i64>) {
    let mut out = Vec::new();
    let mut oldest: Option<i64> = None;
    for p in v.as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(t) = parse_time(&p["updated_at"]) {
            oldest = Some(oldest.map_or(t, |o| o.min(t)));
        }
        let Some(merged_at) = parse_time(&p["merged_at"]) else {
            continue;
        };
        let Some(sha) = p["merge_commit_sha"]
            .as_str()
            .and_then(crate::deploys::commit)
        else {
            continue;
        };
        if !shas.contains(&sha) {
            continue;
        }
        let Some(number) = p["number"]
            .as_i64()
            .filter(|n| (1..=100_000_000).contains(n))
        else {
            continue;
        };
        let Some(title) = clean(&p["title"], MAX_TITLE) else {
            continue;
        };
        out.push(GitPr {
            number,
            title,
            merged_at: Some(merged_at),
            url: url(&p["html_url"]).unwrap_or_default(),
        });
    }
    (out, oldest)
}

/// A conventional-commit prefix (`feat:`, `fix(ui)!:`) and the rest of
/// the title.
fn prefix(title: &str) -> Option<(&str, &str)> {
    let (head, rest) = title.split_once(':')?;
    let word = head.trim_end_matches('!');
    let word = match word.split_once('(') {
        Some((w, scope)) if scope.ends_with(')') => w,
        Some(_) => return None,
        None => word,
    };
    (!word.is_empty() && word.len() <= 12 && word.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| (word, rest.trim()))
}

/// The group of a pull request title, and the title as shown (pure):
/// `feat` is Added, `fix` is Fixed, anything else is Changed; a known
/// prefix is dropped.
#[must_use]
pub fn group_of(title: &str) -> (&'static str, String) {
    const KNOWN: &[&str] = &[
        "feat", "feature", "fix", "bugfix", "hotfix", "perf", "refactor", "chore", "docs", "test",
        "tests", "ci", "build", "style", "revert", "deps",
    ];
    match prefix(title) {
        Some((w, rest)) if KNOWN.contains(&w.to_ascii_lowercase().as_str()) && !rest.is_empty() => {
            let g = match w.to_ascii_lowercase().as_str() {
                "feat" | "feature" => "Added",
                "fix" | "bugfix" | "hotfix" => "Fixed",
                _ => "Changed",
            };
            (g, rest.to_owned())
        }
        _ => ("Changed", title.to_owned()),
    }
}

/// The notes of a read: its pull requests grouped Added, Fixed, Changed
/// (pure). Empty groups are left out.
#[must_use]
pub fn groups(prs: &[GitPr]) -> Vec<NoteGroup> {
    let mut out: Vec<NoteGroup> = ["Added", "Fixed", "Changed"]
        .into_iter()
        .map(|title| NoteGroup {
            title,
            items: Vec::new(),
        })
        .collect();
    for p in prs.iter().take(SHOWN_PRS) {
        let (g, title) = group_of(&p.title);
        if let Some(group) = out.iter_mut().find(|x| x.title == g) {
            group.items.push(NoteItem {
                number: p.number,
                title,
                url: p.url.clone(),
                merged_at: p.merged_at.and_then(rfc3339),
            });
        }
    }
    out.retain(|g| !g.items.is_empty());
    out
}

/// A release name as one path segment of a URL.
fn segment(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn not_found(e: &str) -> bool {
    e.starts_with("GitHub answered 404")
}

/// Read one release's range from GitHub, bounded. The error is why it
/// failed, as GitHub or the transport said.
async fn read<H: Host>(host: &H, gh: &GitHub, p: &ReleaseRange) -> Result<GitRead, String> {
    let GitHub { api, repo, token } = gh;
    let mut out = GitRead::default();
    let mut oldest_committed: Option<i64> = None;
    for page in 1..=MAX_PAGES {
        let u = format!(
            "{api}/repos/{repo}/compare/{}...{}?per_page=100&page={page}",
            p.from, p.to
        );
        let v = host.github_call("GET", &u, token, None).await?;
        let c = compare_page(&v);
        out.commits_total = out.commits_total.max(c.total);
        if page == 1 {
            out.files = c.files;
            out.files_total = c.files_listed;
        }
        if let Some(t) = c.oldest_committed {
            oldest_committed = Some(oldest_committed.map_or(t, |o| o.min(t)));
        }
        let n = c.commits.len();
        for commit in c.commits {
            if out.commits.len() < MAX_COMMITS && !out.commits.iter().any(|x| x.sha == commit.sha) {
                out.commits.push(commit);
            }
        }
        if n < 100 || out.commits.len() >= MAX_COMMITS {
            break;
        }
    }
    out.commits_total = out.commits_total.max(out.commits.len() as i64);
    out.first_commit_at = out.commits.iter().filter_map(|c| c.at).min();
    let shas: HashSet<String> = out.commits.iter().map(|c| c.sha.clone()).collect();
    if !shas.is_empty() {
        let mut found: Vec<GitPr> = Vec::new();
        for page in 1..=MAX_PAGES {
            let u = format!(
                "{api}/repos/{repo}/pulls?state=closed&sort=updated&direction=desc&per_page=100&page={page}"
            );
            let v = host.github_call("GET", &u, token, None).await?;
            let n = v.as_array().map_or(0, Vec::len);
            let (prs, oldest) = merged_in(&v, &shas);
            for pr in prs {
                if !found.iter().any(|x| x.number == pr.number) {
                    found.push(pr);
                }
            }
            // Pull requests updated before the range's first commit were
            // merged before it too.
            let past = matches!((oldest, oldest_committed), (Some(o), Some(c)) if o < c);
            if n < 100 || past {
                break;
            }
        }
        found.sort_by_key(|x| (x.merged_at, x.number));
        out.prs_total = found.len() as i64;
        found.truncate(MAX_PRS);
        out.prs = found;
    }
    // The tag or GitHub release named like the release: optional, so a
    // failure here does not fail the read.
    if p.release.len() <= MAX_TAG {
        let name = segment(&p.release);
        let u = format!("{api}/repos/{repo}/releases/tags/{name}");
        match host.github_call("GET", &u, token, None).await {
            Ok(v) => {
                out.tag = clean(&v["tag_name"], MAX_TAG);
                out.tag_url = url(&v["html_url"]);
            }
            Err(e) if not_found(&e) => {
                let u = format!("{api}/repos/{repo}/git/ref/tags/{name}");
                if let Ok(v) = host.github_call("GET", &u, token, None).await
                    && v["ref"].as_str().is_some()
                {
                    out.tag = Some(p.release.clone());
                }
            }
            Err(_) => {}
        }
    }
    Ok(out)
}

/// The hourly pass for one project (a schedule, not a command: the
/// server's job calls it): read up to [`READS_PER_PASS`] recorded
/// releases not read yet. Gives the failures, for the caller to log; the
/// first one ends the pass (a rate limit would fail the rest too).
pub async fn pass<H: Host>(host: &H, project: &str) -> Vec<String> {
    let Some(gh) = host.github(project) else {
        return Vec::new();
    };
    let Ok(pending) = host
        .meta()
        .release_reads_pending(project, MAX_ATTEMPTS, READS_PER_PASS)
    else {
        return vec!["could not list the releases to read".into()];
    };
    let mut failures = Vec::new();
    for p in pending {
        let now = Utc::now().timestamp();
        match read(host, &gh, &p).await {
            Ok(r) => {
                if let Err(e) = host.meta().store_release_read(project, &p, &r, now) {
                    failures.push(format!("{} ({}): {e}", p.release, p.environment));
                }
            }
            Err(e) => {
                let _ = host.meta().fail_release_read(project, &p, &e, now);
                failures.push(format!("{} ({}): {e}", p.release, p.environment));
                break;
            }
        }
    }
    failures
}

/// The notes of one release of `environment`, or why there are none.
pub fn notes<H: Host>(host: &H, project: &str, environment: &str, release: &str) -> ReleaseNotes {
    let none = |s, r: &str| ReleaseNotes::empty(environment, release, s, r.to_owned());
    if host.github(project).is_none() {
        return none(
            NotesState::NoLink,
            "No GitHub repository linked: set the repository and a read token on the Tickets page.",
        );
    }
    let range = match host.meta().release_range(project, environment, release) {
        Ok(r) => r,
        Err(e) => {
            return none(
                NotesState::Failed,
                &format!("Could not read the range: {e}"),
            );
        }
    };
    let (from, to) = match range {
        Some((Some(from), to)) => (from, to),
        Some((None, _)) => {
            return none(
                NotesState::NoRange,
                "Only the deployed commit is known, not where its range starts: record the release with `openlens-cli release --commit <sha> --from <sha>`.",
            );
        }
        None => {
            return none(
                NotesState::NoRange,
                "No commit range recorded for this release: record it with `openlens-cli release --commit <sha>` in the deploy step.",
            );
        }
    };
    let range = Some(format!("{}..{}", short(&from), short(&to)));
    let state = match host.meta().release_read(project, environment, release) {
        Ok(s) => s.filter(|s| s.from == from && s.to == to),
        Err(e) => {
            return none(
                NotesState::Failed,
                &format!("Could not read the notes: {e}"),
            );
        }
    };
    let pending = |reason: String, state| ReleaseNotes {
        range: range.clone(),
        ..ReleaseNotes::empty(environment, release, state, reason)
    };
    let Some(s) = state else {
        return pending(
            "Not read from GitHub yet: the hourly pass reads it.".into(),
            NotesState::Pending,
        );
    };
    let Some(r) = s.read else {
        let err = s.error.unwrap_or_default();
        return if s.attempts >= MAX_ATTEMPTS {
            pending(
                format!(
                    "GitHub refused {MAX_ATTEMPTS} reads, the last: {err}. Record the release again to retry."
                ),
                NotesState::Failed,
            )
        } else {
            pending(
                format!(
                    "GitHub refused the read ({} of {MAX_ATTEMPTS} tries): {err}. Tried again next hour.",
                    s.attempts
                ),
                NotesState::Pending,
            )
        };
    };
    ReleaseNotes {
        release: release.to_owned(),
        environment: environment.to_owned(),
        state: NotesState::Ready,
        reason: None,
        range,
        tag: r.tag.clone(),
        tag_url: r.tag_url.clone(),
        groups: groups(&r.prs),
        prs: r.prs_total.max(r.prs.len() as i64),
        prs_more: (r.prs_total - r.prs.len() as i64).max(0),
        commits: r.commits_total,
        commits_more: (r.commits_total - r.commits.len() as i64).max(0),
        files: r.files_total,
        files_more: (r.files_total - r.files.len() as i64).max(0),
        first_commit_at: r.first_commit_at.and_then(rfc3339),
        fetched_at: s.fetched_at.and_then(rfc3339),
    }
}

/// The release notes `releases.list` carries: those of the newest
/// releases recorded with a commit range, or why there are none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ListedNotes {
    /// Why no release has notes, when none can (no GitHub link, no range
    /// recorded, or the store failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// The newest releases recorded with a range (at most [`LISTED`]),
    /// newest first, each with its notes or why they are not there yet.
    pub releases: Vec<ReleaseNotes>,
}

/// The notes of the newest releases of `project` recorded with a commit
/// range (at most [`LISTED`]).
pub fn listed<H: Host>(host: &H, project: &str) -> ListedNotes {
    if host.github(project).is_none() {
        return ListedNotes {
            unavailable: Some(
                "Release notes need a GitHub repository and a read token: set them on the Tickets page.".into(),
            ),
            releases: Vec::new(),
        };
    }
    let ranges = match host.meta().release_ranges(project, LISTED as i64) {
        Ok(r) => r,
        Err(e) => {
            return ListedNotes {
                unavailable: Some(format!("Could not read the recorded releases: {e}")),
                releases: Vec::new(),
            };
        }
    };
    if ranges.is_empty() {
        return ListedNotes {
            unavailable: Some(
                "No release recorded with its commit range yet: run `openlens-cli release --commit <sha>` in the deploy step (it finds where the range starts).".into(),
            ),
            releases: Vec::new(),
        };
    }
    ListedNotes {
        unavailable: None,
        releases: ranges
            .iter()
            .map(|r| notes(host, project, &r.environment, &r.release))
            .collect(),
    }
}
