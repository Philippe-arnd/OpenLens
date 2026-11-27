//! The git host link, write side (#575, part of #237, D-096): once a
//! production release has a final verdict (D-067: healthy or regressed,
//! never inconclusive), it is written back to GitHub, where the app's
//! agents already look:
//!
//! - a **commit status** on the deployed commit, context [`CONTEXT`]:
//!   `success` when healthy, `failure` when regressed, linking to the
//!   Releases page when the server knows its public address;
//! - a **note in the GitHub release** named like the release, when the
//!   read (#574) found one: a block between two markers in its body,
//!   replaced on the next write (GitHub releases take no comments);
//! - a **comment on each shipped pull request** (at most
//!   [`MAX_PR_COMMENTS`]), edited when the verdict changes, never repeated;
//! - on a **regressed** verdict, an **issue** with the evidence and the
//!   replay links, filed through the tickets of #245 (one per release: a
//!   later write comments on it, also when the release turns healthy).
//!
//! - **When:** only from the server's hourly pass ([`pass`]), right after
//!   the read; never on a page request. A release is written once its
//!   verdict is final, and again only when the verdict changes.
//! - **Idempotent:** what was written (status, note, comment ids, issue)
//!   is kept in the metadata store (`openlens_meta::release_writes`) and
//!   read before each write.
//! - **Bounds:** [`RELEASES_PER_PASS`] releases and [`COMMENTS_PER_PASS`]
//!   comments per project and pass, releases younger than [`WRITE_DAYS`].
//! - **Failures:** a refusal (403, 404: the token lacks a permission) is
//!   kept and shown, and the other parts are still tried; a rate limit or
//!   an outage ends the pass. A release is tried at most [`MAX_ATTEMPTS`]
//!   times per verdict.
//! - **Safety:** every text sent is built from our own data; anything
//!   that came from outside (release names, pull request titles, error
//!   messages, feature names) is cut and escaped ([`escape`]) so it cannot
//!   format, link, mention anyone or reference an issue. Every URL is the
//!   configured GitHub API base plus our own path. The token is never put
//!   in a message.

use chrono::{DateTime, Utc};
use openlens_meta::release_reads::{GitPr, GitRead};
use openlens_meta::release_writes::{PrComment, ReleaseWrite};
use serde::Serialize;
use serde_json::json;

use crate::commands::Host;
use crate::commands::tickets::{self, Draft, GitHub, Ticket};
use crate::release_git::{self, segment};
use crate::releases::PRODUCTION;
use crate::verdict::{Outcome, Report, Verdict};

/// The commit status context.
pub const CONTEXT: &str = "openlens/release";
/// Pull requests commented per release.
pub const MAX_PR_COMMENTS: usize = release_git::MAX_PRS;
/// Pull request comments written per project and pass (GitHub limits
/// how fast content is created); the rest waits for the next pass.
pub const COMMENTS_PER_PASS: usize = 30;
/// Releases written per project and pass.
pub const RELEASES_PER_PASS: usize = 3;
/// Failed passes of one release and verdict before giving up on it
/// (until the verdict changes).
pub const MAX_ATTEMPTS: i64 = 5;
/// Releases older than this (days since their first event) are not
/// written: a verdict on an old release is no news for its pull requests.
pub const WRITE_DAYS: i64 = 7;
/// Largest GitHub release body edited, in bytes (GitHub's limit is
/// 125 000 characters).
const MAX_RELEASE_BODY: usize = 100_000;
/// Reasons listed in a comment.
const SHOWN_REASONS: usize = 3;
/// Reasons listed in an issue.
const ISSUE_REASONS: usize = 8;
/// Pull requests listed in an issue.
const ISSUE_PRS: usize = 30;
/// Replay links listed in an issue.
const ISSUE_REPLAYS: usize = 3;
/// The markers around our block in a GitHub release's body.
const MARK_START: &str = "<!-- openlens:verdict -->";
const MARK_END: &str = "<!-- /openlens:verdict -->";
/// The marker of our pull request comments.
const MARK_COMMENT: &str = "<!-- openlens:release -->";
/// The only environment written back.
const ENVIRONMENT: &str = PRODUCTION;

/// A KPI a regressed release moved the wrong way.
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    /// Its key, as goals and tickets know it (`error_rate`,
    /// `success:<feature>`…).
    pub key: String,
    /// On the release.
    pub current: f64,
    /// On the release before.
    pub previous: f64,
}

/// What is written for a release: its final verdict and the evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    /// Release name.
    pub release: String,
    /// The release it was compared with.
    pub previous: Option<String>,
    /// Healthy or regressed.
    pub verdict: Verdict,
    /// Why, the worst first.
    pub reasons: Vec<String>,
    /// Error groups first seen in the release.
    pub new_errors: usize,
    /// Hours since its first event.
    pub age_hours: i64,
    /// Its sessions.
    pub sessions: i64,
    /// Sessions to replay (the slowest requests' sessions).
    pub replays: Vec<String>,
    /// The first KPI that got worse, for the issue's success metric.
    pub metric: Option<Metric>,
}

impl Judged {
    /// The final verdict of a report; `None` while it is not judged or
    /// inconclusive.
    #[must_use]
    pub fn of(r: &Report) -> Option<Self> {
        let verdict = r.verdict.filter(|v| *v != Verdict::Inconclusive)?;
        let metric = r
            .checks
            .iter()
            .filter(|c| c.judged && c.outcome == Outcome::Worse)
            .find_map(|c| {
                let key = match c.kind {
                    "kpi" if ["error_rate", "bounce_rate"].contains(&c.key.as_str()) => {
                        c.key.clone()
                    }
                    "feature" => format!("success:{}", c.key),
                    _ => return None,
                };
                Some(Metric {
                    key,
                    current: c.current,
                    previous: c.previous?,
                })
            });
        Some(Self {
            release: r.release.clone(),
            previous: r.previous_release.clone(),
            verdict,
            reasons: r.reasons.clone(),
            new_errors: r.current.new_errors.len(),
            age_hours: r.age_hours,
            sessions: r.current.sessions,
            replays: r
                .checks
                .iter()
                .filter_map(|c| c.exemplar.as_ref()?.session_id.clone())
                .collect(),
            metric,
        })
    }
}

/// Text from outside, made inert for GitHub Markdown (pure): control
/// characters dropped, at most `max` characters, every ASCII punctuation
/// mark escaped (no emphasis, link, image, HTML, code or table), and a
/// word joiner after `@` and `#` so no one is mentioned and no issue is
/// referenced.
#[must_use]
pub fn escape(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars().filter(|c| !c.is_control()).take(max) {
        if ch.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(ch);
        if ch == '@' || ch == '#' {
            out.push('\u{2060}');
        }
    }
    out
}

/// Text from outside for a plain-text field (a status description, an
/// issue title): control characters dropped, at most `max` characters,
/// and a word joiner after `@` and `#`, as in [`escape`].
fn plain(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars().filter(|c| !c.is_control()).take(max) {
        out.push(ch);
        if ch == '@' || ch == '#' {
            out.push('\u{2060}');
        }
    }
    out.chars().take(max).collect()
}

/// A session id fit for a link: short, letters, digits, `-` and `_`.
fn session_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The project's pages on this server (`https://…/p/<id>`), when the host
/// knows its public address.
fn base<H: Host>(host: &H, project: &str) -> Option<String> {
    let b = host.public_url()?;
    let b = b.trim().trim_end_matches('/');
    ((b.starts_with("https://") || b.starts_with("http://"))
        && b.len() <= 200
        && !b.chars().any(|c| c.is_whitespace() || c.is_control()))
    .then(|| format!("{b}/p/{}", segment(project)))
}

fn releases_page(base: &str) -> String {
    format!("{base}/releases?env={ENVIRONMENT}")
}

/// The verdict line and its reasons, in Markdown, every outside string
/// escaped.
fn summary(j: &Judged, base: Option<&str>) -> String {
    let rel = escape(&j.release, 100);
    let against = j
        .previous
        .as_deref()
        .map_or_else(String::new, |p| format!(" against {}", escape(p, 100)));
    let mut md = format!(
        "**OpenLens:** shipped in {rel} ({ENVIRONMENT}), judged **{}**{against}.\n\n",
        j.verdict.label()
    );
    for r in j.reasons.iter().take(SHOWN_REASONS) {
        md.push_str(&format!("- {}\n", escape(r, 200)));
    }
    md.push_str(&match j.new_errors {
        0 => "- No new error.\n".to_owned(),
        1 => "- 1 new error group.\n".to_owned(),
        n => format!("- {n} new error groups.\n"),
    });
    if let Some(b) = base {
        md.push_str(&format!(
            "\n[The release in OpenLens]({})\n",
            releases_page(b)
        ));
    }
    md
}

fn pr_comment(j: &Judged, base: Option<&str>) -> String {
    format!(
        "{MARK_COMMENT}\n{}\n<sub>Edited by OpenLens when the verdict changes.</sub>\n",
        summary(j, base)
    )
}

/// `body` with our block put in, or replaced if it is there (pure).
#[must_use]
pub fn splice(body: &str, block: &str) -> String {
    let ours = format!("{MARK_START}\n{block}\n{MARK_END}");
    if let Some(start) = body.find(MARK_START)
        && let Some(end) = body[start..].find(MARK_END)
    {
        let end = start + end + MARK_END.len();
        return format!("{}{ours}{}", &body[..start], &body[end..]);
    }
    let body = body.trim_end();
    if body.is_empty() {
        ours
    } else {
        format!("{body}\n\n{ours}")
    }
}

/// The commit status: state and description (plain text, 140 at most).
fn status(j: &Judged) -> (&'static str, String) {
    match j.verdict {
        Verdict::Regressed => (
            "failure",
            plain(
                &format!(
                    "regressed: {}",
                    j.reasons.first().map_or("see OpenLens", String::as_str)
                ),
                140,
            ),
        ),
        _ => (
            "success",
            plain(
                &j.previous
                    .as_deref()
                    .map_or_else(|| "healthy".to_owned(), |p| format!("healthy against {p}")),
                140,
            ),
        ),
    }
}

/// The ticket of a regressed release: the one `tickets.file` files, with
/// the fingerprint of the board's "release" finding so a click on the
/// board and this pass share one issue.
fn issue_draft(project: &str, j: &Judged, prs: &[&GitPr], base: Option<&str>) -> Draft {
    let subject = format!("{ENVIRONMENT}:{}", j.release);
    let fp = crate::commands::short_hash(&format!("{project}|release|{subject}"));
    let rel = escape(&j.release, 100);
    let mut md = format!(
        "## Problem\n\n{rel} is judged **regressed**{} in {ENVIRONMENT}, {} h after its first event ({} sessions).\n\n## Evidence (OpenLens)\n\n",
        j.previous
            .as_deref()
            .map_or_else(String::new, |p| format!(" against {}", escape(p, 100))),
        j.age_hours,
        j.sessions
    );
    for r in j.reasons.iter().take(ISSUE_REASONS) {
        md.push_str(&format!("- {}\n", escape(r, 300)));
    }
    if j.new_errors > 0 {
        md.push_str(&format!(
            "- {} error groups first seen in this release.\n",
            j.new_errors
        ));
    }
    md.push_str("\n## Links\n\n");
    match base {
        Some(b) => {
            md.push_str(&format!("- Release: {}\n", releases_page(b)));
            md.push_str(&format!("- Errors: {b}/errors\n- Replays: {b}/replays\n"));
            for s in j
                .replays
                .iter()
                .filter(|s| session_ok(s))
                .take(ISSUE_REPLAYS)
            {
                md.push_str(&format!("- Replay of a slow request: {b}/replays/{s}\n"));
            }
        }
        None => md.push_str(
            "- No links: the OpenLens server has no public address set (`OPENLENS_PUBLIC_URL`).\n",
        ),
    }
    if !prs.is_empty() {
        md.push_str("\n## Shipped in this release\n\n");
        for p in prs.iter().take(ISSUE_PRS) {
            md.push_str(&format!("- #{} {}\n", p.number, escape(&p.title, 120)));
        }
        if prs.len() > ISSUE_PRS {
            md.push_str(&format!("- and {} more\n", prs.len() - ISSUE_PRS));
        }
    }
    md.push_str("\n## Acceptance criteria\n\n- [ ] The cause is found (replay or error trace) and fixed.\n- [ ] The release that ships the fix is recorded, so OpenLens judges it against this one.\n- [ ] No guardrail KPI gets worse.\n\n<sub>Opened by OpenLens from the release verdict. Fingerprint `");
    md.push_str(&fp);
    md.push_str("`.</sub>\n");
    let title = format!(
        "Fix the regression of {}: {}",
        plain(&j.release, 100),
        j.reasons.first().map_or("see OpenLens", String::as_str)
    );
    let (metric, baseline, target, up) = match &j.metric {
        Some(m) => {
            let (_, up, _) = crate::goals::kpi_info(&m.key);
            (Some(m.key.clone()), Some(m.current), Some(m.previous), up)
        }
        None => (None, None, None, true),
    };
    Draft {
        ticket: Ticket {
            fp,
            kind: "release".into(),
            subject,
            title: plain(&title, 200),
            metric,
            baseline,
            baseline_n: j.sessions,
            target,
            up,
            created: Utc::now().format("%Y-%m-%d").to_string(),
            issue: None,
            closed: None,
            shipped_in: None,
            outcome: None,
        },
        markdown: md,
    }
}

/// GitHub refused the call (403 or 404 but a rate limit): the token lacks
/// a permission or cannot see the repository.
fn refused(e: &str) -> bool {
    (e.starts_with("GitHub answered 403") || e.starts_with("GitHub answered 404"))
        && !e.to_ascii_lowercase().contains("rate limit")
}

/// A failure the next call would hit too: a rate limit, an outage or the
/// network. It ends the pass.
fn transient(e: &str) -> bool {
    !e.starts_with("GitHub answered 4")
        || e.starts_with("GitHub answered 429")
        || e.to_ascii_lowercase().contains("rate limit")
}

fn issue_todo(w: &ReleaseWrite, v: &str) -> bool {
    if v == Verdict::Regressed.label() {
        w.issue_verdict.as_deref() != Some(v)
    } else {
        w.issue.is_some() && w.issue_verdict.as_deref() == Some(Verdict::Regressed.label())
    }
}

fn comment_todo(p: &GitPr, comments: &[PrComment], v: &str) -> bool {
    !comments.iter().any(|c| c.pr == p.number && c.verdict == v)
}

/// Whether anything of the release is still to write for verdict `v`.
fn todo(
    w: &ReleaseWrite,
    v: &str,
    sha: &str,
    read: &GitRead,
    prs: &[&GitPr],
    comments: &[PrComment],
) -> bool {
    w.status
        .as_ref()
        .is_none_or(|(sv, ss)| sv != v || ss != sha)
        || (read.tag_url.is_some() && w.note.as_deref() != Some(v))
        || issue_todo(w, v)
        || prs.iter().any(|p| comment_todo(p, comments, v))
}

/// How the writes of one release went.
#[derive(Default)]
struct Wrote {
    /// What was written, for the audit log.
    parts: Vec<String>,
    /// What failed, and why.
    errors: Vec<String>,
    /// A failure the rest of the pass would hit too.
    stop: bool,
}

impl Wrote {
    fn fail(&mut self, what: &str, e: &str) {
        self.stop |= transient(e);
        self.errors.push(format!("{what}: {e}"));
    }

    /// A failure of our own making (an answer we cannot use): it does not
    /// end the pass.
    fn note(&mut self, what: &str, e: &str) {
        self.errors.push(format!("{what}: {e}"));
    }
}

fn https(s: &str) -> Option<String> {
    (s.starts_with("https://")
        && s.len() <= 300
        && !s.chars().any(|c| c.is_control() || c.is_whitespace()))
    .then(|| s.to_owned())
}

/// One release to write, and what it is written from.
struct One<'a> {
    gh: &'a GitHub,
    project: &'a str,
    j: &'a Judged,
    sha: &'a str,
    read: &'a GitRead,
    prs: &'a [&'a GitPr],
    base: Option<&'a str>,
}

/// Write what is still to write of one release, part by part, keeping
/// each part in `w` (and each comment in the store) as GitHub takes it.
/// `budget` is the pull request comments the pass may still write.
async fn write_one<H: Host>(
    host: &H,
    one: &One<'_>,
    comments: &mut Vec<PrComment>,
    w: &mut ReleaseWrite,
    budget: &mut usize,
) -> Wrote {
    let One {
        gh,
        project,
        j,
        sha,
        read,
        prs,
        base,
    } = *one;
    let GitHub { api, repo, token } = gh;
    let v = j.verdict.label();
    let mut out = Wrote::default();

    // The commit status.
    if w.status
        .as_ref()
        .is_none_or(|(sv, ss)| sv != v || ss != sha)
    {
        let (state, description) = status(j);
        let mut body = json!({ "state": state, "context": CONTEXT, "description": description });
        if let Some(b) = base {
            body["target_url"] = json!(releases_page(b));
        }
        let u = format!("{api}/repos/{repo}/statuses/{sha}");
        match host.github_call("POST", &u, token, Some(body)).await {
            Ok(_) => {
                w.status = Some((v.to_owned(), sha.to_owned()));
                out.parts.push(format!("commit status {state}"));
            }
            Err(e) => out.fail("commit status", &e),
        }
    }
    if out.stop {
        return out;
    }

    // The note in the GitHub release.
    if read.tag_url.is_some() && w.note.as_deref() != Some(v) && j.release.len() <= 200 {
        let u = format!("{api}/repos/{repo}/releases/tags/{}", segment(&j.release));
        match host.github_call("GET", &u, token, None).await {
            Ok(r) => {
                let id = r["id"].as_i64().filter(|n| *n > 0);
                let body = r["body"].as_str().unwrap_or("");
                match id {
                    None => out.note("release note", "GitHub gave no release id"),
                    Some(_) if body.len() > MAX_RELEASE_BODY => out.note(
                        "release note",
                        "the GitHub release's text is too long to edit",
                    ),
                    Some(id) => {
                        let new = splice(body, &summary(j, base));
                        let u = format!("{api}/repos/{repo}/releases/{id}");
                        match host
                            .github_call("PATCH", &u, token, Some(json!({ "body": new })))
                            .await
                        {
                            Ok(_) => {
                                w.note = Some(v.to_owned());
                                out.parts.push("release note".into());
                            }
                            Err(e) => out.fail("release note", &e),
                        }
                    }
                }
            }
            Err(e) => out.fail("release note", &e),
        }
    }
    if out.stop {
        return out;
    }

    // The issue of a regressed release, or the news that it is healthy now.
    if issue_todo(w, v) {
        if j.verdict == Verdict::Regressed {
            match tickets::file(host, project, issue_draft(project, j, prs, base)).await {
                Ok(t) => {
                    w.issue = t.issue.and_then(|(n, u)| {
                        Some((i64::try_from(n).ok()?, https(&u).unwrap_or_default()))
                    });
                    w.issue_verdict = Some(v.to_owned());
                    out.parts.push(match &w.issue {
                        Some((n, _)) => format!("issue #{n}"),
                        None => "issue".into(),
                    });
                }
                Err(e) => out.fail("issue", &e),
            }
        } else if let Some((n, _)) = w.issue.clone() {
            let u = format!("{api}/repos/{repo}/issues/{n}/comments");
            let body = format!(
                "{}\nThis release no longer looks regressed; close this issue once you agree.\n",
                summary(j, base)
            );
            match host
                .github_call("POST", &u, token, Some(json!({ "body": body })))
                .await
            {
                Ok(_) => {
                    w.issue_verdict = Some(v.to_owned());
                    out.parts.push(format!("comment on issue #{n}"));
                }
                Err(e) => out.fail("issue", &e),
            }
        }
    }
    if out.stop {
        return out;
    }

    // One comment per shipped pull request, edited when the verdict changes.
    let text = pr_comment(j, base);
    let mut commented = 0;
    let left: Vec<&GitPr> = prs
        .iter()
        .copied()
        .filter(|p| comment_todo(p, comments, v))
        .collect();
    for p in left {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let existing = comments.iter().find(|c| c.pr == p.number).map(|c| c.id);
        let body = json!({ "body": text });
        let edited = match existing {
            Some(id) => {
                let u = format!("{api}/repos/{repo}/issues/comments/{id}");
                match host
                    .github_call("PATCH", &u, token, Some(body.clone()))
                    .await
                {
                    Ok(_) => Some(Ok(id)),
                    // Deleted on GitHub: write it again.
                    Err(e) if e.starts_with("GitHub answered 404") => None,
                    Err(e) => Some(Err(e)),
                }
            }
            None => None,
        };
        let r = match edited {
            Some(r) => r,
            None => {
                let u = format!("{api}/repos/{repo}/issues/{}/comments", p.number);
                host.github_call("POST", &u, token, Some(body))
                    .await
                    .and_then(|r| {
                        r["id"]
                            .as_i64()
                            .filter(|n| *n > 0)
                            .ok_or_else(|| "GitHub gave no comment id".to_owned())
                    })
            }
        };
        match r {
            Ok(id) => {
                let c = PrComment {
                    pr: p.number,
                    id,
                    verdict: v.to_owned(),
                };
                if let Err(e) =
                    host.meta()
                        .save_release_write_comment(project, ENVIRONMENT, &j.release, &c)
                {
                    out.fail("pull request comment", &e.to_string());
                    out.stop = true;
                    break;
                }
                comments.retain(|x| x.pr != p.number);
                comments.push(c);
                commented += 1;
            }
            Err(e) => {
                out.fail(&format!("comment on #{}", p.number), &e);
                // One refusal says it for every pull request.
                break;
            }
        }
    }
    if commented > 0 {
        out.parts.push(format!("{commented} pull request comments"));
    }
    out
}

/// The hourly pass for one project (a schedule, not a command: the
/// server's job calls it right after [`release_git::pass`]): judge the
/// production releases and write the final verdicts back. Gives the
/// failures, for the caller to log.
pub async fn pass<H: Host>(host: &H, project: &str) -> Vec<String> {
    if host.github(project).is_none() {
        return Vec::new();
    }
    match host
        .meta()
        .release_ranges(project, release_git::LISTED as i64)
    {
        Ok(r) if r.iter().any(|r| r.environment == ENVIRONMENT) => {}
        Ok(_) => return Vec::new(),
        Err(e) => return vec![format!("could not list the recorded releases: {e}")],
    }
    let reports = match crate::commands::releases::judged(host, project, ENVIRONMENT).await {
        Ok(r) => r,
        Err(e) => return vec![format!("could not judge the releases: {e}")],
    };
    let judged: Vec<Judged> = reports.iter().filter_map(Judged::of).collect();
    write(host, project, &judged).await
}

/// Write the final verdicts in `judged` back to GitHub (the pass, once
/// the releases are judged): up to [`RELEASES_PER_PASS`] recorded
/// production releases, newest first, whose range was read and that have
/// something left to write. Gives the failures, for the caller to log.
pub async fn write<H: Host>(host: &H, project: &str, judged: &[Judged]) -> Vec<String> {
    let Some(gh) = host.github(project) else {
        return Vec::new();
    };
    let ranges = match host
        .meta()
        .release_ranges(project, release_git::LISTED as i64)
    {
        Ok(r) => r,
        Err(e) => return vec![format!("could not list the recorded releases: {e}")],
    };
    let base = base(host, project);
    let mut failures = Vec::new();
    let mut budget = COMMENTS_PER_PASS;
    let mut written = 0;
    for r in ranges.iter().filter(|r| r.environment == ENVIRONMENT) {
        if written >= RELEASES_PER_PASS {
            break;
        }
        let Some(j) = judged.iter().find(|j| j.release == r.release) else {
            continue;
        };
        if j.age_hours > WRITE_DAYS * 24 {
            continue;
        }
        let Some(sha) = crate::deploys::commit(&r.to) else {
            continue;
        };
        let read = match host.meta().release_read(project, ENVIRONMENT, &r.release) {
            Ok(Some(s)) if s.from == r.from && s.to == r.to => s.read,
            Ok(_) => None,
            Err(e) => {
                failures.push(format!("{}: {e}", r.release));
                continue;
            }
        };
        // The pull requests and the GitHub release come from the read.
        let Some(read) = read else {
            continue;
        };
        let (stored, mut comments) = match (
            host.meta().release_write(project, ENVIRONMENT, &r.release),
            host.meta()
                .release_write_comments(project, ENVIRONMENT, &r.release),
        ) {
            (Ok(s), Ok(c)) => (s, c),
            (Err(e), _) | (_, Err(e)) => {
                failures.push(format!("{}: {e}", r.release));
                continue;
            }
        };
        let v = j.verdict.label();
        let mut w = stored.unwrap_or_default();
        if w.target != v {
            w = ReleaseWrite {
                target: v.to_owned(),
                attempts: 0,
                error: None,
                denied: false,
                written_at: None,
                ..w
            };
        }
        let prs: Vec<&GitPr> = read.prs.iter().take(MAX_PR_COMMENTS).collect();
        if !todo(&w, v, &sha, &read, &prs, &comments) || w.attempts >= MAX_ATTEMPTS {
            continue;
        }
        written += 1;
        let one = One {
            gh: &gh,
            project,
            j,
            sha: &sha,
            read: &read,
            prs: &prs,
            base: base.as_deref(),
        };
        let out = write_one(host, &one, &mut comments, &mut w, &mut budget).await;
        let now = Utc::now().timestamp();
        w.tried_at = Some(now);
        if out.errors.is_empty() {
            w.error = None;
            w.denied = false;
            if !todo(&w, v, &sha, &read, &prs, &comments) {
                w.written_at = Some(now);
            }
        } else {
            w.attempts += 1;
            w.denied = out
                .errors
                .iter()
                .all(|e| e.split_once(": ").is_some_and(|(_, e)| refused(e)));
            w.error = Some(out.errors.join("; "));
        }
        if let Err(e) = host
            .meta()
            .save_release_write(project, ENVIRONMENT, &r.release, &w)
        {
            failures.push(format!("{}: {e}", r.release));
        }
        if !out.parts.is_empty() {
            let _ = host.meta().audit(
                "openlens",
                Some(project),
                "release.write_back",
                &format!(
                    "{} ({ENVIRONMENT}): {v}: {}",
                    plain(&r.release, 100),
                    out.parts.join(", ")
                ),
            );
        }
        failures.extend(out.errors.iter().map(|e| format!("{}: {e}", r.release)));
        if out.stop {
            break;
        }
    }
    failures
}

/// Where the write-back of a release stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteState {
    /// Nothing is written back: no GitHub link, or not production.
    Off,
    /// Nothing written yet: waiting for a final verdict and the read.
    Waiting,
    /// Partly written; the rest on the next pass.
    Writing,
    /// Everything written for the verdict.
    Written,
    /// GitHub refused a write: the token lacks a permission.
    Denied,
    /// A write failed for another reason.
    Failed,
}

/// What was written back to GitHub for a release, or why nothing was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteBack {
    /// Where it stands.
    pub state: WriteState,
    /// In plain words.
    pub reason: String,
    /// The verdict written, or being written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// The commit status set (`healthy` or `regressed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Whether the GitHub release carries the verdict.
    pub release_note: bool,
    /// Pull requests commented.
    pub comments: i64,
    /// The issue opened on a regressed verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    /// When everything was written (RFC 3339, UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written_at: Option<String>,
}

/// The permissions the write-back needs, as said to the person.
pub const SCOPES: &str = "a fine-grained token with Commit statuses, Contents, Issues and Pull requests set to read and write (or a classic token with the repo scope)";

/// What was written back for one release of `environment`, or why
/// nothing was.
pub fn shown<H: Host>(host: &H, project: &str, environment: &str, release: &str) -> WriteBack {
    let off = |state, reason: String| WriteBack {
        state,
        reason,
        verdict: None,
        status: None,
        release_note: false,
        comments: 0,
        issue_url: None,
        written_at: None,
    };
    if host.github(project).is_none() {
        return off(
            WriteState::Off,
            "Write-back to GitHub is off: no repository and token are linked (set them on the Tickets page).".into(),
        );
    }
    if environment != ENVIRONMENT {
        return off(
            WriteState::Off,
            format!("Only {ENVIRONMENT} releases are written back to GitHub."),
        );
    }
    let (w, comments) = match (
        host.meta().release_write(project, environment, release),
        host.meta()
            .release_write_comments(project, environment, release),
    ) {
        (Ok(w), Ok(c)) => (w, c),
        (Err(e), _) | (_, Err(e)) => {
            return off(
                WriteState::Failed,
                format!("Could not read what was written back: {e}"),
            );
        }
    };
    let Some(w) = w else {
        return off(
            WriteState::Waiting,
            "Nothing written to GitHub yet: the verdict is written back once it is healthy or regressed and the pull requests are read (hourly).".into(),
        );
    };
    let n = comments.iter().filter(|c| c.verdict == w.target).count() as i64;
    let mut parts = Vec::new();
    if let Some((v, _)) = &w.status {
        parts.push(format!("commit status {CONTEXT} ({v})"));
    }
    if w.note.is_some() {
        parts.push("the GitHub release's notes".into());
    }
    if n > 0 {
        parts.push(format!("{n} pull request comments"));
    }
    if let Some((i, _)) = &w.issue {
        parts.push(format!("issue #{i}"));
    }
    let parts = if parts.is_empty() {
        "nothing yet".to_owned()
    } else {
        parts.join(", ")
    };
    let tries = if w.attempts >= MAX_ATTEMPTS {
        format!("Gave up after {MAX_ATTEMPTS} tries, until the verdict changes.")
    } else {
        format!(
            "Tried {} of {MAX_ATTEMPTS} times; tried again next hour.",
            w.attempts
        )
    };
    let (state, reason) = match &w.error {
        Some(e) if w.denied => (
            WriteState::Denied,
            format!(
                "GitHub refused part of the write-back ({e}). It needs {SCOPES}. Written: {parts}. {tries}"
            ),
        ),
        Some(e) => (
            WriteState::Failed,
            format!("Writing to GitHub failed ({e}). Written: {parts}. {tries}"),
        ),
        None if w.written_at.is_some() => (
            WriteState::Written,
            format!("Written to GitHub ({}): {parts}.", w.target),
        ),
        None if w.status.is_some() || w.note.is_some() || n > 0 => (
            WriteState::Writing,
            format!(
                "Partly written to GitHub ({}): {parts}; the rest next hour.",
                w.target
            ),
        ),
        None => (WriteState::Waiting, "Nothing written to GitHub yet.".into()),
    };
    WriteBack {
        state,
        reason,
        verdict: Some(w.target.clone()),
        status: w.status.map(|s| s.0),
        release_note: w.note.is_some(),
        comments: n,
        issue_url: w.issue.map(|i| i.1).filter(|u| !u.is_empty()),
        written_at: w
            .written_at
            .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
            .map(|d| d.to_rfc3339()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_block_in_a_release_body_is_replaced_not_repeated() {
        let once = splice("## What's new\n\n- export", "healthy");
        assert!(once.starts_with("## What's new"));
        let twice = splice(&once, "regressed");
        assert!(twice.contains("regressed") && !twice.contains("healthy"));
        assert_eq!(twice.matches(MARK_START).count(), 1);
        assert!(twice.contains("- export"));
    }

    #[test]
    fn a_rate_limit_is_not_a_refusal() {
        assert!(refused(
            "GitHub answered 403: Resource not accessible by personal access token"
        ));
        assert!(!refused("GitHub answered 403: API rate limit exceeded"));
        assert!(transient("GitHub answered 403: API rate limit exceeded"));
        assert!(!transient("GitHub answered 422: No commit found for SHA"));
    }
}
