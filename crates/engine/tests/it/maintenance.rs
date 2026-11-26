//! Planned maintenance (#563, D-076): an outage inside a window sends no
//! alert and does not count against availability; the same outage
//! lasting past the window's end is announced at the end.

use std::sync::Mutex;

use openlens_engine::commands::alerts::{self, Seen, track_at};
use openlens_engine::commands::maintenance::{self, ScheduleInput, WindowInput};
use openlens_engine::commands::{Caller, Change, Command, Host, tickets};
use openlens_engine::{Board, EngineError, Filters};
use openlens_featuremap::FeatureMap;
use openlens_meta::maintenance::NewWindow;
use openlens_meta::{Meta, Project};
use openlens_query::{Limits, QueryEngine};

/// A host that keeps the alerts it is asked to send.
struct TestHost {
    dir: tempfile::TempDir,
    meta: Meta,
    query: QueryEngine,
    sent: Mutex<Vec<(String, String)>>,
}

impl TestHost {
    fn new() -> (Self, Project) {
        let dir = tempfile::tempdir().unwrap();
        let meta = Meta::in_memory().unwrap();
        let p = meta.create_project("shop", None).unwrap();
        let query = QueryEngine::new(dir.path(), Limits::default());
        let host = Self {
            dir,
            meta,
            query,
            sent: Mutex::default(),
        };
        (host, p)
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().unwrap().clone()
    }
}

impl Host for TestHost {
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
        title: &str,
        body: &str,
        _: &str,
        _: bool,
    ) -> Vec<alerts::Delivery> {
        self.sent
            .lock()
            .unwrap()
            .push((title.to_owned(), body.to_owned()));
        Vec::new()
    }
    fn github(&self, _: &str) -> Option<tickets::GitHub> {
        None
    }
    async fn github_call(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        Err("no GitHub in this test".into())
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

fn down(failing: bool) -> Seen<'static> {
    Seen {
        rule: "down",
        failing,
        cause: "https://shop.example: HTTP 503",
        wait: 120,
    }
}

async fn schedule(h: &TestHost, p: &Project, minutes: u32) -> maintenance::MaintenanceWindow {
    let input = ScheduleInput {
        project: p.id.clone(),
        minutes: Some(minutes),
        text: "Database migration".into(),
        ..ScheduleInput::default()
    };
    maintenance::Schedule::run(h, &Caller::default(), input)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_outage_inside_a_window_is_not_announced() {
    let (h, p) = TestHost::new();
    let w = schedule(&h, &p, 30).await;
    assert_eq!(w.state, "running");
    let st = alerts::GetStatus::run(
        &h,
        &Caller::default(),
        alerts::ProjectInput {
            project: p.id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        st.state, "maintenance",
        "the component shows under maintenance"
    );
    // Down for most of the window, back before it ends.
    for t in [w.start + 60, w.start + 600, w.start + 1200] {
        track_at(&h, &p.id, &p.name, &down(true), t).await;
    }
    assert!(
        h.meta.open_incident(&p.id, "down").unwrap().is_some(),
        "the incident still opens"
    );
    track_at(&h, &p.id, &p.name, &down(false), w.end - 60).await;
    assert!(
        h.sent().is_empty(),
        "no alert, no reminder, no all-clear: {:?}",
        h.sent()
    );
    // Without the window, the same outage is announced.
    let (h2, p2) = TestHost::new();
    for t in [w.start + 60, w.start + 600] {
        track_at(&h2, &p2.id, &p2.name, &down(true), t).await;
    }
    assert!(!h2.sent().is_empty());
}

#[tokio::test]
async fn an_outage_lasting_past_the_window_is_announced_at_its_end() {
    let (h, p) = TestHost::new();
    let w = schedule(&h, &p, 30).await;
    for t in [w.start + 60, w.start + 600, w.end - 60] {
        track_at(&h, &p.id, &p.name, &down(true), t).await;
    }
    assert!(h.sent().is_empty());
    track_at(&h, &p.id, &p.name, &down(true), w.end).await;
    let sent = h.sent();
    assert_eq!(sent.len(), 1, "announced once, when the window ends");
    assert!(sent[0].1.contains("planned maintenance"), "{:?}", sent[0]);
    // Then the usual: the all-clear when it is back.
    track_at(&h, &p.id, &p.name, &down(false), w.end + 120).await;
    assert!(h.sent().len() > sent.len());
}

#[tokio::test]
async fn ending_a_window_early_announces_from_then() {
    let (h, p) = TestHost::new();
    let w = schedule(&h, &p, 60).await;
    let now = chrono::Utc::now().timestamp();
    track_at(&h, &p.id, &p.name, &down(true), now - 300).await;
    let input = WindowInput {
        project: p.id.clone(),
        id: w.id,
    };
    let ended = maintenance::EndNow::run(&h, &Caller::default(), input)
        .await
        .unwrap();
    assert!(ended.end < w.end);
    track_at(&h, &p.id, &p.name, &down(true), ended.end + 60).await;
    assert_eq!(h.sent().len(), 1);
    // A window that ran cannot be cancelled.
    let again = WindowInput {
        project: p.id.clone(),
        id: w.id,
    };
    assert!(
        maintenance::Cancel::run(&h, &Caller::default(), again)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn availability_leaves_maintenance_out_and_says_how_much() {
    let (h, p) = TestHost::new();
    let now = chrono::Utc::now().timestamp();
    // Two hours of checks a minute apart; down for the first half hour of
    // the second hour, inside a window.
    let start = now - 7200;
    for i in 0..120 {
        let at = start + i * 60;
        let ok = !(60..90).contains(&i);
        h.meta
            .record_check(
                &p.id,
                at,
                ok,
                Some(if ok { 200 } else { 503 }),
                Some(40),
                None,
            )
            .unwrap();
    }
    let raw = 90.0 / 120.0;
    let comps = vec!["production".to_owned()];
    h.meta
        .add_maintenance(
            &p.id,
            &NewWindow {
                starts_at: start + 60 * 60 - 30,
                ends_at: start + 90 * 60,
                components: &comps,
                text: "Database migration",
                created_by: "test",
                now,
            },
        )
        .unwrap();
    let u = alerts::GetUptime::run(
        &h,
        &Caller::default(),
        alerts::ProjectInput {
            project: p.id.clone(),
        },
    )
    .await
    .unwrap();
    let day = u.avail.0.unwrap();
    assert!(day > raw, "the failed checks in the window are not counted");
    assert!(day >= 1.0, "every check outside the window succeeded");
    assert!(u.maintenance_left_out.0 > 0 && u.maintenance_left_out.1 >= u.maintenance_left_out.0);
    assert_eq!(u.maintenance.len(), 1);
    assert_eq!(u.maintenance[0].state, "ended");
}
