//! The board's query string (D-055, D-060), shared by the board page and
//! `GET /api/projects/{id}/board`. Moved out of `api.rs` as is.

use openlens_engine::Filters;
use serde::Deserialize;

use crate::api::FilterQuery;

/// The board's query: the filters, and `window=fixed` for the fixed 7-,
/// 14- and 30-day windows instead of the release window (D-055), or
/// `since=<release>` or `from=&to=` (`YYYY-MM-DD`, the project's days) for
/// a past release or a date range (D-060). The board page reads the same.
#[derive(Deserialize, Default)]
pub(crate) struct BoardQuery {
    #[serde(flatten)]
    pub(crate) f: FilterQuery,
    window: Option<String>,
    since: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

impl BoardQuery {
    /// The board's filters: the window asked for, else the release window
    /// unless `window=fixed`.
    pub(crate) fn filters(&self, tz: &str) -> Result<Filters, openlens_engine::EngineError> {
        let f = self.f.filters(tz);
        let asked = openlens_engine::Window::asked(
            self.since.as_deref(),
            self.from.as_deref(),
            self.to.as_deref(),
            &f.tz,
            chrono::Utc::now(),
        )?;
        Ok(Filters {
            window: asked.unwrap_or(match self.window.as_deref().map(str::trim) {
                Some("fixed") => openlens_engine::Window::Fixed,
                _ => openlens_engine::Window::Release,
            }),
            ..f
        })
    }
}
