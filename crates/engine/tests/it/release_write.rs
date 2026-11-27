//! The git host link, write side (#575, D-096): a release's final verdict
//! is written back to GitHub once (commit status, the GitHub release's
//! notes, one comment per pull request, an issue when regressed), a
//! changed verdict edits what is there, a refusing token is recorded and
//! given up on, and outside text is escaped.

use std::sync::Mutex;

use openlens_engine::commands::alerts;
use openlens_engine::commands::deploys::{RecordRelease, RecordReleaseInput};
use openlens_engine::commands::release_notes::{Notes, NotesInput};
use openlens_engine::commands::{Caller, Change, Command, Host, tickets};
use openlens_engine::release_git;
use openlens_engine::release_write::{self, CONTEXT, Judged, MAX_ATTEMPTS, WriteState};
use openlens_engine::verdict::Verdict;
use openlens_engine::{Board, EngineError, Filters};
use openlens_featuremap::FeatureMap;
use openlens_meta::{Meta, Project};
use openlens_query::{Limits, QueryEngine};
use serde_json::{Value, json};

const API: &str = "https://api.github.test";
const FROM: &str = "1111111111111111111111111111111111111111";
const C1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const C2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HOSTILE: &str =
    "feat: @everyone <img src=x onerror=alert(1)> [click](https://evil.example) fixes #1";

/// One call made to the fake GitHub.
#[derive(Debug, Clone)]
struct Call {
    method: String,
    url: String,
    body: Option<Value>,
}

/// A host whose GitHub reads from fixtures and takes (or refuses) writes,
/// recording every call.
struct WriteHost {
    dir: tempfile::TempDir,
    meta: Meta,
    query: QueryEngine,
    deny_writes: bool,
    calls: Mutex<Vec<Call>>,
    next_id: Mutex<i64>,
}

impl WriteHost {
    fn new(deny_writes: bool) -> (Self, Project) {
        let dir = tempfile::tempdir().unwrap();
        let meta = Meta::in_memory().unwrap();
        let p = meta.create_project("shop", None).unwrap();
        let query = QueryEngine::new(dir.path(), Limits::default());
        let h = Self {
            dir,
            meta,
            query,
            deny_writes,
            calls: Mutex::default(),
            next_id: Mutex::new(1000),
        };
        (h, p)
    }

    fn writes(&self) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method != "GET")
            .cloned()
            .collect()
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

fn read_answer(url: &str) -> Result<Value, String> {
    if url.contains("/compare/") {
        return Ok(json!({
            "total_commits": 2,
            "commits": [
                { "sha": C1, "commit": { "author": { "date": "2026-11-20T10:00:00Z" } } },
                { "sha": C2, "commit": { "author": { "date": "2026-11-21T09:00:00Z" } } }
            ],
            "files": [ { "filename": "src/export.rs" } ]
        }));
    }
    if url.contains("/pulls?") {
        return Ok(json!([
            { "number": 7, "title": HOSTILE, "merged_at": "2026-11-21T09:00:00Z",
              "merge_commit_sha": C2, "html_url": "https://github.com/o/r/pull/7",
              "updated_at": "2026-11-21T09:00:00Z" },
            { "number": 8, "title": "fix: keep filters on export", "merged_at": "2026-11-20T10:05:00Z",
              "merge_commit_sha": C1, "html_url": "https://github.com/o/r/pull/8",
              "updated_at": "2026-11-20T10:05:00Z" }
        ]));
    }
    if url.contains("/releases/tags/") {
        return Ok(json!({
            "id": 55, "tag_name": "v1.4", "body": "## What's new\n\n- CSV export",
            "html_url": "https://github.com/o/r/releases/tag/v1.4"
        }));
    }
    Err("GitHub answered 404: Not Found".into())
}

impl Host for WriteHost {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn query(&self) -> &QueryEngine {
        &self.query
    }
    async fn project(&self, id: &str) -> Option<Project> {
        self.meta.project(id).ok().flatten()
    }
    async fn feature_map(&self, _: &str) -> Option<(FeatureMap, i64)> {
        None
    }
    async fn board(
        &self,
        _: &str,
        _: &Filters,
        _: Option<(&FeatureMap, i64)>,
    ) -> openlens_engine::Result<Board> {
        Err(EngineError::InvalidFilter("no board in this test"))
    }
    fn changes(&self, _: &str, _: &[(String, String)], _: i64) -> Vec<Change> {
        Vec::new()
    }
    fn alert_level(&self, _: &str, _: &str) -> String {
        "notify".into()
    }
    async fn send_alert(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: bool,
    ) -> Vec<alerts::Delivery> {
        Vec::new()
    }
    fn github(&self, _: &str) -> Option<tickets::GitHub> {
        Some(tickets::GitHub {
            api: API.into(),
            repo: "o/r".into(),
            token: "secret-token".into(),
        })
    }
    async fn github_call(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        assert!(url.starts_with(API), "only the configured API: {url}");
        assert_eq!(token, "secret-token");
        self.calls.lock().unwrap().push(Call {
            method: method.to_owned(),
            url: url.to_owned(),
            body: body.clone(),
        });
        if method == "GET" {
            return read_answer(url);
        }
        if self.deny_writes {
            return Err(
                "GitHub answered 403: Resource not accessible by personal access token".into(),
            );
        }
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        if url.ends_with("/repos/o/r/issues") {
            return Ok(json!({ "number": 12, "html_url": "https://github.com/o/r/issues/12" }));
        }
        Ok(json!({ "id": *id }))
    }
    fn set_github_token(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn random_hex(&self, bytes: usize) -> String {
        "0".repeat(bytes * 2)
    }
    fn data_dir(&self) -> &std::path::Path {
        self.dir.path()
    }
    fn public_url(&self) -> Option<String> {
        Some("https://lens.example".into())
    }
}

/// A release recorded with its range and read from GitHub.
async fn read_release(deny_writes: bool) -> (WriteHost, Project) {
    let (h, p) = WriteHost::new(deny_writes);
    let input = RecordReleaseInput {
        project: p.id.clone(),
        release: "v1.4".into(),
        environment: None,
        commit: Some(C2.into()),
        from: Some(FROM.into()),
        ..RecordReleaseInput::default()
    };
    RecordRelease::run(&h, &Caller::default(), input)
        .await
        .unwrap();
    assert!(release_git::pass(&h, &p.id).await.is_empty());
    (h, p)
}

fn judged(verdict: Verdict) -> Judged {
    Judged {
        release: "v1.4".into(),
        previous: Some("v1.3".into()),
        verdict,
        reasons: vec![match verdict {
            Verdict::Regressed => "Sessions with errors: 2.0% → 9.0% `<script>` @admin".into(),
            _ => "Nothing got worse than v1.3.".into(),
        }],
        new_errors: 0,
        age_hours: 30,
        sessions: 500,
        replays: vec!["s-1".into(), "../../evil".into()],
        metric: None,
    }
}

async fn write_back(h: &WriteHost, p: &Project) -> release_write::WriteBack {
    let input = NotesInput {
        project: p.id.clone(),
        release: "v1.4".into(),
        environment: None,
    };
    Notes::run(h, &Caller::default(), input)
        .await
        .unwrap()
        .write_back
        .unwrap()
}

fn comment_posts(w: &[Call]) -> usize {
    w.iter()
        .filter(|c| {
            c.method == "POST" && c.url.contains("/issues/") && c.url.ends_with("/comments")
        })
        .count()
}

#[tokio::test]
async fn a_healthy_verdict_is_written_once_and_a_second_pass_writes_nothing() {
    let (h, p) = read_release(false).await;
    assert_eq!(write_back(&h, &p).await.state, WriteState::Waiting);
    assert!(h.writes().is_empty(), "reading writes nothing");

    let failures = release_write::write(&h, &p.id, &[judged(Verdict::Healthy)]).await;
    assert!(failures.is_empty(), "{failures:?}");
    let w = h.writes();
    let status = w
        .iter()
        .find(|c| c.url.ends_with(&format!("/statuses/{C2}")))
        .expect("a commit status on the deployed commit");
    let body = status.body.clone().unwrap();
    assert_eq!(body["context"], CONTEXT);
    assert_eq!(body["state"], "success");
    assert!(
        body["target_url"]
            .as_str()
            .unwrap()
            .starts_with("https://lens.example/")
    );
    let note = w
        .iter()
        .find(|c| c.method == "PATCH" && c.url.ends_with("/releases/55"))
        .expect("the GitHub release's notes are edited");
    let text = note.body.as_ref().unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("CSV export"),
        "the release's own text is kept"
    );
    assert!(text.contains("healthy"));
    assert_eq!(comment_posts(&w), 2, "one comment per shipped pull request");
    assert!(
        !w.iter().any(|c| c.url.ends_with("/repos/o/r/issues")),
        "no issue for a healthy release"
    );
    let shown = write_back(&h, &p).await;
    assert_eq!(shown.state, WriteState::Written);
    assert_eq!(shown.comments, 2);
    assert!(
        !serde_json::to_string(&shown)
            .unwrap()
            .contains("secret-token")
    );

    // A restart or a second pass: nothing new.
    let before = h.calls();
    assert!(
        release_write::write(&h, &p.id, &[judged(Verdict::Healthy)])
            .await
            .is_empty()
    );
    assert_eq!(h.calls(), before, "nothing written twice");
    // No final verdict: nothing written either.
    assert!(release_write::write(&h, &p.id, &[]).await.is_empty());
    assert_eq!(h.calls(), before);
}

#[tokio::test]
async fn a_changed_verdict_edits_the_comments_and_opens_one_issue() {
    let (h, p) = read_release(false).await;
    release_write::write(&h, &p.id, &[judged(Verdict::Healthy)]).await;
    let first = h.writes().len();
    let posted = comment_posts(&h.writes());

    let failures = release_write::write(&h, &p.id, &[judged(Verdict::Regressed)]).await;
    assert!(failures.is_empty(), "{failures:?}");
    let w: Vec<Call> = h.writes().into_iter().skip(first).collect();
    let status = w.iter().find(|c| c.url.contains("/statuses/")).unwrap();
    assert_eq!(status.body.as_ref().unwrap()["state"], "failure");
    assert_eq!(
        comment_posts(&h.writes()),
        posted,
        "comments are edited, not repeated"
    );
    assert_eq!(
        w.iter()
            .filter(|c| c.method == "PATCH" && c.url.contains("/issues/comments/"))
            .count(),
        posted
    );
    let issues = |w: &[Call]| {
        w.iter()
            .filter(|c| c.url.ends_with("/repos/o/r/issues"))
            .count()
    };
    assert_eq!(issues(&w), 1, "a regressed release opens an issue");
    let shown = write_back(&h, &p).await;
    assert_eq!(shown.state, WriteState::Written);
    assert!(shown.issue_url.is_some());

    // The same verdict again: nothing more, not a second issue.
    let before = h.calls();
    release_write::write(&h, &p.id, &[judged(Verdict::Regressed)]).await;
    assert_eq!(h.calls(), before);
    assert_eq!(issues(&h.writes()), 1);
}

#[tokio::test]
async fn a_token_without_write_access_is_recorded_and_given_up_on() {
    let (h, p) = read_release(true).await;
    let failures = release_write::write(&h, &p.id, &[judged(Verdict::Healthy)]).await;
    assert!(failures.iter().any(|f| f.contains("403")), "{failures:?}");
    assert!(
        failures.iter().all(|f| !f.contains("secret-token")),
        "the token is never in a message"
    );
    let shown = write_back(&h, &p).await;
    assert_eq!(shown.state, WriteState::Denied);
    assert!(shown.reason.contains("403"));
    for _ in 1..MAX_ATTEMPTS {
        release_write::write(&h, &p.id, &[judged(Verdict::Healthy)]).await;
    }
    let before = h.calls();
    release_write::write(&h, &p.id, &[judged(Verdict::Healthy)]).await;
    assert_eq!(h.calls(), before, "given up: no call every hour forever");
    assert!(write_back(&h, &p).await.reason.contains("Gave up"));
}

#[tokio::test]
async fn outside_text_sent_to_github_is_escaped() {
    let (h, p) = read_release(false).await;
    release_write::write(&h, &p.id, &[judged(Verdict::Regressed)]).await;
    let w = h.writes();
    let issue = w
        .iter()
        .find(|c| c.url.ends_with("/repos/o/r/issues"))
        .expect("an issue");
    let body = issue.body.as_ref().unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(body.contains("click"), "the title is listed");
    for raw in [
        "@everyone",
        "@admin",
        " <img",
        "](https://evil",
        "`<script>`",
        "#1",
    ] {
        assert!(!body.contains(raw), "{raw:?} reached GitHub as is:\n{body}");
    }
    assert!(body.contains("/replays/s-1"));
    assert!(
        !body.contains("/replays/.."),
        "a session id that is not one is left out"
    );
    for c in &w {
        let text = c.body.as_ref().map(Value::to_string).unwrap_or_default();
        assert!(!text.contains("@everyone") && !text.contains("@admin"));
    }
}
