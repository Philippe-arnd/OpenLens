//! Per-page hotspots: views, how far people scroll, what they click and
//! where they rage-click (last 30 days). A lean heatmap: no screenshots,
//! computed from events the SDK already sends.

use serde::Serialize;

use crate::{Filters, Result, float, int, text};
use openlens_query::QueryEngine;

/// A clicked element.
#[derive(Debug, Clone, Serialize)]
pub struct Hotspot {
    /// Feature, else visible text, else tag.
    pub label: String,
    /// Clicks.
    pub clicks: i64,
    /// Rage clicks (3 fast clicks on the same element).
    pub rage: i64,
}

/// One page.
#[derive(Debug, Clone, Serialize)]
pub struct PageStats {
    /// Path.
    pub path: String,
    /// Page views.
    pub views: i64,
    /// Distinct users.
    pub users: i64,
    /// Median deepest scroll, % of the page.
    pub median_scroll: f64,
    /// Share of page visits that reached half the page.
    pub reach_half: f64,
    /// Share that reached the end (90%+).
    pub reach_end: f64,
    /// Most clicked elements.
    pub hotspots: Vec<Hotspot>,
}

/// The 15 most viewed pages with their scroll depth and click hotspots.
pub async fn pages(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<PageStats>> {
    let w = f.clause(30)?;
    let views_sql = format!(
        "SELECT path, count(*) AS views, count(DISTINCT distinct_id) AS users FROM {src} \
         WHERE {w} AND event = '$pageview' AND path IS NOT NULL GROUP BY path ORDER BY views DESC LIMIT 15"
    );
    let scroll_sql = format!(
        "SELECT path, approx_median(d) AS med, avg(CASE WHEN d >= 50 THEN 1.0 ELSE 0.0 END) AS half, \
         avg(CASE WHEN d >= 90 THEN 1.0 ELSE 0.0 END) AS reach_end FROM ( \
           SELECT path, session_id, max(CAST(regexp_match(properties, '\"\\$depth\":([0-9]+)')[1] AS DOUBLE)) AS d \
           FROM {src} WHERE {w} AND event = '$scroll' AND path IS NOT NULL GROUP BY path, session_id) s \
         WHERE d IS NOT NULL GROUP BY path"
    );
    let clicks_sql = format!(
        "SELECT path, label, count(*) AS clicks, sum(rage) AS rage FROM ( \
           SELECT path, coalesce(feature, regexp_match(properties, '\"\\$text\":\"([^\"]{{1,60}})\"')[1], \
                  regexp_match(properties, '\"\\$tag\":\"([a-z0-9-]+)\"')[1], '?') AS label, \
                  CASE WHEN properties LIKE '%\"$rage\":true%' THEN 1 ELSE 0 END AS rage \
           FROM {src} WHERE {w} AND event = '$click' AND path IS NOT NULL) c \
         GROUP BY path, label ORDER BY clicks DESC LIMIT 400"
    );
    let (views, scroll, clicks) = futures::join!(
        q.sql(project, &views_sql),
        q.sql(project, &scroll_sql),
        q.sql(project, &clicks_sql)
    );
    let (scroll, clicks) = (scroll?.rows, clicks?.rows);
    Ok(views?
        .rows
        .iter()
        .map(|r| {
            let path = text(r, "path").unwrap_or_default();
            let s = scroll
                .iter()
                .find(|x| text(x, "path").as_deref() == Some(path.as_str()));
            let hotspots = clicks
                .iter()
                .filter(|x| text(x, "path").as_deref() == Some(path.as_str()))
                .take(5)
                .map(|x| Hotspot {
                    label: text(x, "label").unwrap_or_default(),
                    clicks: int(x, "clicks"),
                    rage: int(x, "rage"),
                })
                .collect();
            PageStats {
                views: int(r, "views"),
                users: int(r, "users"),
                median_scroll: s.map_or(-1.0, |x| float(x, "med")),
                reach_half: s.map_or(0.0, |x| float(x, "half")),
                reach_end: s.map_or(0.0, |x| float(x, "reach_end")),
                hotspots,
                path,
            }
        })
        .collect())
}

/// Click positions on one page, bucketed for a heatmap: 50 columns (2% of
/// the viewport width each) by 20-pixel rows, last 30 days.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClickMap {
    /// Page path.
    pub path: String,
    /// `(column 0..50, row, clicks, rage clicks)`, busiest first.
    pub cells: Vec<(i64, i64, i64, i64)>,
    /// Clicks with a position.
    pub clicks: i64,
    /// Median viewport width of those clicks, in pixels.
    pub viewport: i64,
}

/// Columns of a click map.
pub const MAP_COLUMNS: i64 = 50;
/// Row height of a click map, in pixels.
pub const MAP_ROW: i64 = 20;

/// The click map of `path`.
pub async fn click_map(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    path: &str,
) -> Result<ClickMap> {
    let w = f.clause(30)?;
    let p = crate::lit(path)?;
    let base = format!(
        "SELECT CAST(regexp_match(properties, '\"\\$x\":([0-9.]+)')[1] AS DOUBLE) AS x, \
                CAST(regexp_match(properties, '\"\\$y\":([0-9]+)')[1] AS DOUBLE) AS y, \
                CAST(regexp_match(properties, '\"\\$vw\":([0-9]+)')[1] AS DOUBLE) AS vw, \
                CASE WHEN properties LIKE '%\"$rage\":true%' THEN 1 ELSE 0 END AS rage \
         FROM {src} WHERE {w} AND event = '$click' AND path = {p}"
    );
    let cells_sql = format!(
        "SELECT CAST(least(floor(x * {MAP_COLUMNS}), {MAP_COLUMNS} - 1) AS BIGINT) AS cx, CAST(floor(y / {MAP_ROW}) AS BIGINT) AS cy, \
                count(*) AS n, sum(rage) AS rage \
         FROM ({base}) c WHERE x IS NOT NULL AND y IS NOT NULL AND x <= 1 AND y < 30000 \
         GROUP BY 1, 2 ORDER BY n DESC LIMIT 3000"
    );
    let totals_sql = format!(
        "SELECT count(*) AS n, approx_median(vw) AS vw FROM ({base}) c WHERE x IS NOT NULL AND y IS NOT NULL"
    );
    let (cells, totals) = futures::join!(q.sql(project, &cells_sql), q.sql(project, &totals_sql));
    let t = totals?.rows;
    Ok(ClickMap {
        path: path.to_owned(),
        cells: cells?
            .rows
            .iter()
            .map(|r| (int(r, "cx"), int(r, "cy"), int(r, "n"), int(r, "rage")))
            .collect(),
        clicks: t.first().map_or(0, |r| int(r, "n")),
        viewport: t.first().map_or(0, |r| float(r, "vw") as i64),
    })
}

/// Recent sessions that viewed `path` (newest first, at most 200), to find a
/// recording to draw a heatmap over.
pub async fn sessions_on_path(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    path: &str,
) -> Result<Vec<String>> {
    let w = f.clause(30)?;
    let sql = format!(
        "SELECT session_id, max(timestamp) AS t FROM {src} WHERE {w} AND event = '$pageview' AND path = {} \
         AND session_id IS NOT NULL GROUP BY session_id ORDER BY t DESC LIMIT 200",
        crate::lit(path)?
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter_map(|r| text(r, "session_id"))
        .collect())
}
