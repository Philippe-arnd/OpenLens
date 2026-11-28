//! Next actions (#715, D-114): an agent proposes, a person approves, and
//! the ticket is filed once, however many times either clicks.

use std::sync::Mutex;

use openlens_engine::commands::actions::{self, ApproveInput, NextInput, ProposeInput, State};
use openlens_engine::commands::{Caller, Change, Command, Host, alerts, tickets};
use openlens_engine::{Board, Filters, Insight};
use openlens_featuremap::FeatureMap;
use openlens_meta::{Meta, Project};
use openlens_query::{Limits, QueryEngine};

/// A host whose board shows one card and whose GitHub keeps the calls.
struct Shop {
    dir: tempfile::TempDir,
    meta: Meta,
    query: QueryEngine,
    calls: Mutex<Vec<(String, String)>>,
}

impl Shop {
    fn new() -> (Self, Project) {
        let dir = tempfile::tempdir().unwrap();
        let meta = Meta::in_memory().unwrap();
        let p = meta.create_project("shop", None).unwrap();
        let query = QueryEngine::new(dir.path(), Limits::default());
        let host = Self {
            dir,
            meta,
            query,
            calls: Mutex::default(),
        };
        (host, p)
    }

    fn posts(&self) -> usize {
        let calls = self.calls.lock().unwrap();
        calls.iter().filter(|(m, _)| m == "POST").count()
    }
}

impl Host for Shop {
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
        Ok(Board {
            insights: vec![Insight {
                kind: "friction",
                title: "Half of the checkouts are abandoned".into(),
                evidence: "120 of 240 started checkouts finish".into(),
                next_step: "Watch the replays of the abandoned ones".into(),
                score: 4.0,
                subject: "checkout".into(),
                figure: None,
            }],
            ..Board::default()
        })
    }
    fn changes(&self, _: &str, _: &[(String, String)], _: i64) -> Vec<Change> {
        Vec::new()
    }
    fn alert_level(&self, _: &str, _: &str) -> String {
        "log".into()
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
            api: "https://github.invalid".into(),
            repo: "acme/shop".into(),
            token: "t".into(),
        })
    }
    async fn github_call(
        &self,
        method: &str,
        url: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let mut calls = self.calls.lock().unwrap();
        calls.push((method.to_owned(), url.to_owned()));
        let n = calls.len();
        Ok(serde_json::json!({
            "number": n,
            "html_url": format!("https://github.invalid/acme/shop/issues/{n}"),
        }))
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

async fn next(h: &Shop, p: &Project) -> Vec<actions::Action> {
    let input = NextInput {
        project: p.id.clone(),
        state: None,
    };
    actions::Next::run(h, &Caller::default(), input)
        .await
        .unwrap()
        .actions
}

#[tokio::test]
async fn propose_then_approve_moves_the_state_and_files_one_ticket() {
    let (h, p) = Shop::new();
    let list = next(&h, &p).await;
    let card = list
        .iter()
        .find(|a| a.finding.subject == "friction:checkout")
        .unwrap();
    assert_eq!(card.state, State::New);
    let id = card.id.clone();

    let propose = || ProposeInput {
        project: p.id.clone(),
        id: id.clone(),
        note: Some("I will look at the replays first".into()),
        host: "https://analytics.example".into(),
    };
    for _ in 0..2 {
        let a = actions::Propose::run(&h, &Caller::default(), propose())
            .await
            .unwrap();
        assert_eq!(a.state, State::Proposed);
    }
    assert_eq!(h.posts(), 0, "a proposal posts nothing");
    let proposed: Vec<_> = next(&h, &p)
        .await
        .into_iter()
        .filter(|a| a.id == id)
        .collect();
    assert_eq!(proposed.len(), 1, "proposing twice keeps one action");
    assert_eq!(proposed[0].state, State::Proposed);
    assert!(proposed[0].draft.is_some(), "the drafted ticket is kept");

    let approve = || ApproveInput {
        project: p.id.clone(),
        id: id.clone(),
        host: "https://analytics.example".into(),
    };
    for _ in 0..2 {
        let a = actions::Approve::run(&h, &Caller::default(), approve())
            .await
            .unwrap();
        assert_eq!(a.state, State::Approved);
        assert!(a.ticket.as_ref().is_some_and(|t| t.url.is_some()));
    }
    assert_eq!(h.posts(), 1, "approving twice files one issue");
    let input = tickets::ProjectInput {
        project: p.id.clone(),
    };
    let filed = tickets::List::run(&h, &Caller::default(), input)
        .await
        .unwrap()
        .tickets;
    assert_eq!(filed.iter().filter(|t| t.fp == id).count(), 1);
    let after = next(&h, &p).await;
    assert!(
        after
            .iter()
            .any(|a| a.id == id && a.state == State::Approved)
    );
}
