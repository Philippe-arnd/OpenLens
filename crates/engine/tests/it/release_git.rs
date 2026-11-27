//! The git host link, read side (#574, D-093): a release recorded with
//! its range is read from GitHub once, by the hourly pass, and its notes
//! are then served from the store; GitHub's answers are bounded; a
//! failing GitHub is retried a few times, then given up. Its changed files
//! are matched once to the features whose code paths they are in (#576).

use std::collections::HashSet;
use std::sync::Mutex;

use openlens_engine::commands::alerts;
use openlens_engine::commands::deploys::{RecordRelease, RecordReleaseInput};
use openlens_engine::commands::release_notes::{Notes, NotesInput};
use openlens_engine::commands::{Caller, Change, Command, Host, tickets};
use openlens_engine::release_features::{self, TouchedState};
use openlens_engine::release_git::{
    self, MAX_ATTEMPTS, MAX_COMMITS, MAX_FILES, NotesState, ReleaseNotes,
};
use openlens_engine::verdict::{FeatureUse, Side, Verdict, compare};
use openlens_engine::{Board, EngineError, Filters};
use openlens_featuremap::FeatureMap;
use openlens_meta::{Meta, Project};
use openlens_query::{Limits, QueryEngine};
use serde_json::{Value, json};

const FROM: &str = "1111111111111111111111111111111111111111";
const C1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const C2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A host whose GitHub answers from fixtures, or always fails.
struct GitHost {
    dir: tempfile::TempDir,
    meta: Meta,
    query: QueryEngine,
    linked: bool,
    failing: bool,
    calls: Mutex<Vec<String>>,
}

impl GitHost {
    fn new(linked: bool, failing: bool) -> (Self, Project) {
        let dir = tempfile::tempdir().unwrap();
        let meta = Meta::in_memory().unwrap();
        let p = meta.create_project("shop", None).unwrap();
        let query = QueryEngine::new(dir.path(), Limits::default());
        let h = Self {
            dir,
            meta,
            query,
            linked,
            failing,
            calls: Mutex::default(),
        };
        (h, p)
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

/// What the fixture GitHub answers for `url`.
fn answer(url: &str) -> Result<Value, String> {
    if url.contains("/compare/") {
        return Ok(json!({
            "total_commits": 2,
            "commits": [
                { "sha": C1, "commit": {
                    "author": { "name": "Alice Example", "email": "alice@example.com", "date": "2026-11-20T10:00:00Z" },
                    "committer": { "date": "2026-11-20T10:05:00Z" } },
                  "author": { "login": "alice-gh" } },
                { "sha": C2, "commit": {
                    "author": { "name": "Bob Example", "email": "bob@example.com", "date": "2026-11-21T09:00:00Z" },
                    "committer": { "date": "2026-11-21T09:00:00Z" } } }
            ],
            "files": [ { "filename": "src/export.rs" }, { "filename": "src/filters.rs" },
                       { "filename": "src/export/csv.rs" }, { "filename": "README.md" } ]
        }));
    }
    if url.contains("/pulls?") {
        return Ok(json!([
            { "number": 7, "title": "feat(export): CSV export", "merged_at": "2026-11-21T09:00:00Z",
              "merge_commit_sha": C2, "html_url": "https://github.com/o/r/pull/7",
              "updated_at": "2026-11-21T09:00:00Z", "user": { "login": "bob-gh" } },
            { "number": 8, "title": "fix: keep filters on export", "merged_at": "2026-11-20T10:05:00Z",
              "merge_commit_sha": C1, "html_url": "https://github.com/o/r/pull/8",
              "updated_at": "2026-11-20T10:05:00Z", "user": { "login": "alice-gh" } },
            { "number": 9, "title": "feat: another branch", "merged_at": "2026-11-21T08:00:00Z",
              "merge_commit_sha": "cccccccccccccccccccccccccccccccccccccccc",
              "html_url": "https://github.com/o/r/pull/9", "updated_at": "2026-11-21T08:00:00Z" },
            { "number": 10, "title": "closed, not merged", "merged_at": null,
              "merge_commit_sha": C1, "html_url": "https://github.com/o/r/pull/10",
              "updated_at": "2026-11-20T08:00:00Z" }
        ]));
    }
    if url.contains("/releases/tags/") {
        return Err("GitHub answered 404: Not Found".into());
    }
    if url.contains("/git/ref/tags/") {
        return Ok(json!({ "ref": "refs/tags/v1.4" }));
    }
    Err("GitHub answered 404: Not Found".into())
}

impl Host for GitHost {
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
        self.linked.then(|| tickets::GitHub {
            api: "https://api.github.test".into(),
            repo: "o/r".into(),
            token: "t".into(),
        })
    }
    async fn github_call(
        &self,
        method: &str,
        url: &str,
        _: &str,
        _: Option<Value>,
    ) -> Result<Value, String> {
        assert_eq!(method, "GET", "the read side only reads");
        self.calls.lock().unwrap().push(url.to_owned());
        if self.failing {
            return Err("GitHub answered 403: API rate limit exceeded".into());
        }
        answer(url)
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
}

async fn record(h: &GitHost, p: &Project, from: Option<&str>) {
    let input = RecordReleaseInput {
        project: p.id.clone(),
        release: "v1.4".into(),
        environment: None,
        commit: Some(C2.into()),
        from: from.map(str::to_owned),
        ..RecordReleaseInput::default()
    };
    RecordRelease::run(h, &Caller::default(), input)
        .await
        .unwrap();
}

async fn notes(h: &GitHost, p: &Project) -> ReleaseNotes {
    let input = NotesInput {
        project: p.id.clone(),
        release: "v1.4".into(),
        environment: None,
    };
    Notes::run(h, &Caller::default(), input).await.unwrap()
}

#[tokio::test]
async fn a_recorded_range_is_read_once_and_its_notes_group_the_merged_pull_requests() {
    let (h, p) = GitHost::new(true, false);
    record(&h, &p, Some(FROM)).await;
    assert_eq!(notes(&h, &p).await.state, NotesState::Pending);
    assert_eq!(h.calls(), 0, "recording a release calls nothing");

    assert!(release_git::pass(&h, &p.id).await.is_empty());
    let n = notes(&h, &p).await;
    assert_eq!(n.state, NotesState::Ready);
    let titles: Vec<&str> = n
        .groups
        .iter()
        .flat_map(|g| g.items.iter().map(|i| i.title.as_str()))
        .collect();
    assert!(titles.contains(&"CSV export"));
    assert!(titles.contains(&"keep filters on export"));
    assert!(
        !titles
            .iter()
            .any(|t| t.contains("another branch") || t.contains("not merged")),
        "only pull requests merged into the range: {titles:?}"
    );
    let added = n.groups.iter().find(|g| g.title == "Added").unwrap();
    let fixed = n.groups.iter().find(|g| g.title == "Fixed").unwrap();
    assert!(added.items.iter().any(|i| i.number == 7));
    assert!(fixed.items.iter().any(|i| i.number == 8));
    assert!(n.tag.is_some(), "the tag named like the release is found");
    assert!(n.first_commit_at.is_some());

    // What attribution (#576) reads, with no call.
    let files = h
        .meta
        .release_read_files(&p.id, "production", "v1.4")
        .unwrap();
    assert!(files.contains(&"src/export.rs".to_owned()));

    // No author is kept or shown.
    let shown = serde_json::to_string(&n).unwrap();
    for who in ["alice", "bob", "Alice", "Bob"] {
        assert!(!shown.contains(who), "{who} leaked into the notes");
    }

    // Read once: the next passes and every read call nothing more.
    let calls = h.calls();
    assert!(release_git::pass(&h, &p.id).await.is_empty());
    let _ = notes(&h, &p).await;
    assert_eq!(h.calls(), calls);
}

#[tokio::test]
async fn a_failing_github_is_retried_a_few_times_then_given_up() {
    let (h, p) = GitHost::new(true, true);
    record(&h, &p, Some(FROM)).await;
    let first = release_git::pass(&h, &p.id).await;
    assert!(
        !first.is_empty(),
        "the failure is given to the caller to log"
    );
    let n = notes(&h, &p).await;
    assert_eq!(n.state, NotesState::Pending, "retried next hour");
    assert!(n.reason.unwrap_or_default().contains("rate limit"));
    for _ in 1..MAX_ATTEMPTS {
        let _ = release_git::pass(&h, &p.id).await;
    }
    assert_eq!(notes(&h, &p).await.state, NotesState::Failed);
    let calls = h.calls();
    assert!(release_git::pass(&h, &p.id).await.is_empty());
    assert_eq!(h.calls(), calls, "given up: no more calls");
}

#[tokio::test]
async fn without_a_link_or_a_range_the_notes_say_why() {
    let (h, p) = GitHost::new(false, false);
    record(&h, &p, Some(FROM)).await;
    assert_eq!(notes(&h, &p).await.state, NotesState::NoLink);
    assert!(release_git::pass(&h, &p.id).await.is_empty());
    assert_eq!(h.calls(), 0);

    // The project's first release: its range has no start.
    let (h, p) = GitHost::new(true, false);
    record(&h, &p, None).await;
    let n = notes(&h, &p).await;
    assert_eq!(n.state, NotesState::NoRange);
    assert!(n.reason.is_some());
    let _ = release_git::pass(&h, &p.id).await;
    assert_eq!(h.calls(), 0, "nothing to read without a range");
}

#[test]
fn hostile_github_answers_are_bounded() {
    let long = "x".repeat(10_000);
    let commits: Vec<Value> = (0..400)
        .map(|i| json!({ "sha": format!("{i:040x}"), "commit": { "author": { "date": "2026-11-20T10:00:00Z" } } }))
        .chain([
            json!({ "sha": "not a sha" }),
            json!({ "sha": long }),
        ])
        .collect();
    let files: Vec<Value> = (0..1_000)
        .map(|i| json!({ "filename": format!("src/f{i}.rs") }))
        .chain([
            json!({ "filename": long }),
            json!({ "filename": "a\u{0}b" }),
        ])
        .collect();
    let c = release_git::compare_page(
        &json!({ "total_commits": 400, "commits": commits, "files": files }),
    );
    assert!(c.commits.len() <= MAX_COMMITS);
    assert!(c.files.len() <= MAX_FILES);
    assert!(
        c.files_listed > c.files.len() as i64,
        "the rest is counted for \"and K more\""
    );
    assert!(c.files.iter().all(|f| f.len() < 1_000 && !f.contains('\0')));

    let shas: HashSet<String> = [C1.to_owned()].into();
    let pulls = json!([
        { "number": 1, "title": long, "merged_at": "2026-11-20T10:00:00Z", "merge_commit_sha": C1,
          "html_url": "javascript:alert(1)" },
        { "number": -5, "title": "negative", "merged_at": "2026-11-20T10:00:00Z", "merge_commit_sha": C1 },
        { "number": 2, "title": "\u{7}", "merged_at": "2026-11-20T10:00:00Z", "merge_commit_sha": C1 }
    ]);
    let (prs, _) = release_git::merged_in(&pulls, &shas);
    assert_eq!(prs.len(), 1, "a bad number or an empty title is dropped");
    assert!(prs[0].title.chars().count() < long.len());
    assert!(prs[0].url.is_empty(), "only https links are kept");
    // Not a list at all.
    assert!(
        release_git::merged_in(&json!({ "message": "x" }), &shas)
            .0
            .is_empty()
    );
}

/// A map whose `export` feature owns `src/export/**` and `filters` one
/// file; `src/export.rs` and `README.md` are in no feature.
const MAP_WITH_PATHS: &str = "features:\n  - key: checkout\n    name: Checkout\n    success: paid\n    entry: { routes: ['/checkout'] }\n    paths: ['src/checkout/**']\n  - key: export\n    name: Export\n    success: exported\n    entry: { routes: ['/export'] }\n    paths: ['src/export/**']\n  - key: filters\n    name: Filters\n    success: filtered\n    entry: { routes: ['/filters'] }\n    paths: ['src/filters.rs']\n";

fn feature_use(key: &str, sessions: i64, succeeded: i64) -> FeatureUse {
    FeatureUse {
        feature: key.into(),
        sessions,
        succeeded,
    }
}

fn release_side(release: &str, touched: Vec<String>) -> Side {
    Side {
        release: release.into(),
        users: 150,
        sessions: 500,
        bounced: 100,
        error_sessions: 10,
        task_sessions: 250,
        tasks: 500,
        avg_secs: 60.0,
        features: vec![
            feature_use("checkout", 200, 150),
            feature_use("export", 200, 150),
            feature_use("filters", 200, 150),
        ],
        touched,
        ..Side::default()
    }
}

#[tokio::test]
async fn a_release_that_changes_export_lists_it_as_touched_and_its_verdict_compares_it_first() {
    let (h, p) = GitHost::new(true, false);
    h.meta.save_feature_map(&p.id, MAP_WITH_PATHS).unwrap();
    record(&h, &p, Some(FROM)).await;
    assert!(release_git::pass(&h, &p.id).await.is_empty());

    let n = notes(&h, &p).await;
    let t = n.touched.expect("ready notes carry the features touched");
    assert_eq!(t.state, TouchedState::Ready);
    let keys: Vec<&str> = t.features.iter().map(|f| f.key.as_str()).collect();
    assert!(keys.contains(&"export"), "{keys:?}");
    assert!(keys.contains(&"filters"));
    assert!(!keys.contains(&"checkout"), "no file of checkout changed");
    assert!(t.unmatched > 0 && t.unmatched < t.files);
    assert!(
        t.note
            .clone()
            .unwrap_or_default()
            .contains("match no feature")
    );

    // Matched once and kept: what the verdict and the feature page read.
    let by = release_features::by_release(&h.meta, &p.id, "production");
    let (_, fs) = by.iter().find(|(r, _)| r == "v1.4").unwrap();
    let touched: Vec<String> = fs.iter().map(|f| f.key.clone()).collect();
    assert!(touched.contains(&"export".to_owned()));
    let history = openlens_engine::History::load(&h.meta, &p.id, "production");
    assert!(
        history
            .touched
            .iter()
            .any(|(r, fs)| r == "v1.4" && fs.iter().any(|f| f.key == "export")),
        "the feature page's Releases tab reads it"
    );

    // Export's and checkout's success drop: export, touched, is judged
    // first, before every other check, and gives the first reason.
    let mut current = release_side("v1.4", touched);
    current.features[0] = feature_use("checkout", 200, 60);
    current.features[1] = feature_use("export", 200, 60);
    let previous = release_side("v1.3", Vec::new());
    let r = compare("production", &current, Some(&previous), 30, &[]);
    let first = r.checks.first().unwrap();
    assert_eq!((first.kind, first.key.as_str()), ("feature", "export"));
    let at = |key: &str| r.checks.iter().position(|c| c.key == key).unwrap();
    assert!(at("export") < at("error_rate"));
    assert!(at("export") < at("checkout"));
    assert_eq!(r.verdict, Some(Verdict::Regressed));
    assert!(r.reasons[0].contains("export"), "{:?}", r.reasons);

    // A later pass matches and calls nothing again.
    let calls = h.calls();
    assert!(release_git::pass(&h, &p.id).await.is_empty());
    assert_eq!(h.calls(), calls);
}

#[tokio::test]
async fn files_in_no_feature_and_a_map_without_paths_say_so() {
    // Paths declared, none matching: no feature, and the gap is counted.
    let (h, p) = GitHost::new(true, false);
    h.meta
        .save_feature_map(&p.id, &MAP_WITH_PATHS.replace("src/", "lib/"))
        .unwrap();
    record(&h, &p, Some(FROM)).await;
    let _ = release_git::pass(&h, &p.id).await;
    let t = notes(&h, &p).await.touched.unwrap();
    assert_eq!(t.state, TouchedState::Ready);
    assert!(t.features.is_empty());
    assert!(t.files > 0);
    assert_eq!(t.unmatched, t.files);
    let gap = format!("{} changed files match no feature", t.files);
    assert!(t.note.unwrap_or_default().contains(&gap));

    // No paths at all: not "N files match no feature", but how to add them.
    let (h, p) = GitHost::new(true, false);
    let bare: String = MAP_WITH_PATHS
        .lines()
        .filter(|l| !l.contains("paths:"))
        .map(|l| format!("{l}\n"))
        .collect();
    h.meta.save_feature_map(&p.id, &bare).unwrap();
    record(&h, &p, Some(FROM)).await;
    let _ = release_git::pass(&h, &p.id).await;
    let t = notes(&h, &p).await.touched.unwrap();
    assert_eq!(t.state, TouchedState::NoPaths);
    assert!(t.features.is_empty() && t.note.is_none());
    assert!(t.reason.unwrap_or_default().contains("openlens-cli rules"));
}
