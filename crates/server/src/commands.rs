//! The server as the host of engine commands (#494, D-042): the stores,
//! and the cached board and timeline the commands read.

use openlens_engine::commands::alerts::Delivery;
use openlens_engine::commands::tickets::GitHub;
use openlens_engine::commands::{Caller, Change, Host};
use openlens_featuremap::FeatureMap;
use openlens_meta::{Meta, Project};
use openlens_query::QueryEngine;

use crate::AppState;

impl Host for AppState {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn query(&self) -> &QueryEngine {
        &self.query
    }

    async fn project(&self, id: &str) -> Option<Project> {
        let meta = self.meta.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || meta.project(&id))
            .await
            .ok()?
            .ok()?
    }

    async fn feature_map(&self, project: &str) -> Option<(FeatureMap, i64)> {
        crate::load_map(self, project).await
    }

    async fn board(
        &self,
        project: &str,
        f: &openlens_engine::Filters,
        map: Option<(&FeatureMap, i64)>,
    ) -> openlens_engine::Result<openlens_engine::Board> {
        crate::boards::board(self, project, f, map).await
    }

    fn changes(&self, project: &str, releases: &[(String, String)], days: i64) -> Vec<Change> {
        crate::timeline::collect(self, project, releases, days)
            .into_iter()
            .map(|m| Change {
                day: m.day,
                kind: m.kind,
                text: m.text,
            })
            .collect()
    }

    fn alert_level(&self, project: &str, env: &str) -> String {
        crate::environments::current(self, project)
            .rules
            .level(env)
            .to_owned()
    }

    async fn send_alert(
        &self,
        project: &str,
        title: &str,
        body: &str,
        rule: &str,
        resolved: bool,
    ) -> Vec<Delivery> {
        crate::alerts::deliver(self, project, title, body, rule, resolved).await
    }

    fn github(&self, project: &str) -> Option<GitHub> {
        crate::tickets::github(self, project).map(|(api, repo, token)| GitHub { api, repo, token })
    }

    async fn github_call(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let (method, url, token) = (method.to_owned(), url.to_owned(), token.to_owned());
        tokio::task::spawn_blocking(move || crate::tickets::gh(&method, &url, &token, body))
            .await
            .map_err(|e| e.to_string())?
    }

    fn set_github_token(&self, token: &str) -> Result<(), String> {
        let s = crate::secrets::seal(&self.data_dir, token)?;
        let _ = self.meta.set_setting("github_token", &s);
        Ok(())
    }

    fn random_hex(&self, bytes: usize) -> String {
        crate::cookieless::random_hex(bytes)
    }

    fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    fn capacity_inputs(&self) -> openlens_engine::capacity::Inputs {
        crate::capacity::inputs(self)
    }
}

/// The caller of a command made from a dashboard request: the viewer's
/// own time zone when it is valid.
pub(crate) fn caller(state: &AppState, headers: &axum::http::HeaderMap) -> Caller {
    Caller {
        tz: crate::auth::viewer(state, headers)
            .and_then(|v| v.tz)
            .filter(|t| t.parse::<chrono_tz::Tz>().is_ok()),
    }
}
