//! The server as the host of engine commands (#494, D-042): the stores,
//! and the cached board and timeline the commands read.

use openlens_engine::commands::alerts::Delivery;
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
