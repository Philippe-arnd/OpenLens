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
    let q = &q.with_params(&f.params()?);
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
    let q = &q.with_params(&f.params()?);
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
    let q = &q.with_params(&f.params()?);
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

/// Answers to one micro-survey, last 30 days.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SurveyResults {
    /// Survey key.
    pub key: String,
    /// `(answer, count)`, most frequent first.
    pub answers: Vec<(String, i64)>,
    /// People who closed it without answering.
    pub dismissed: i64,
    /// Mean answer, for ratings (1–5).
    pub mean: Option<f64>,
    /// Mean rating per canonical role, for ratings.
    pub by_role: Vec<(String, f64, i64)>,
    /// Latest free-text answers (at most 10), newest first.
    pub latest_text: Vec<String>,
}

/// Results of every survey seen in the last 30 days.
pub async fn survey_results(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<SurveyResults>> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(30)?;
    let base = format!(
        "SELECT regexp_match(properties, '\"\\$survey\":\"([a-z0-9_-]+)\"')[1] AS k, \
                coalesce(regexp_match(properties, '\"\\$answer\":\"((?:[^\"\\\\]|\\\\.){{0,500}})\"')[1], \
                         regexp_match(properties, '\"\\$answer\":([0-9]+)')[1]) AS a, \
                CASE WHEN properties LIKE '%\"$dismissed\":true%' THEN 1 ELSE 0 END AS d, \
                role, timestamp \
         FROM {src} WHERE {w} AND event = '$survey'"
    );
    let counts_sql = format!(
        "SELECT k, a, d, count(*) AS n FROM ({base}) s WHERE k IS NOT NULL GROUP BY k, a, d ORDER BY n DESC LIMIT 2000"
    );
    let roles_sql = format!(
        "SELECT k, coalesce(role, 'not identified') AS role, avg(TRY_CAST(a AS DOUBLE)) AS m, count(*) AS n \
         FROM ({base}) s WHERE k IS NOT NULL AND d = 0 AND TRY_CAST(a AS DOUBLE) BETWEEN 1 AND 5 GROUP BY k, role"
    );
    let text_sql = format!(
        "SELECT k, a FROM ({base}) s WHERE k IS NOT NULL AND d = 0 AND a IS NOT NULL \
         AND TRY_CAST(a AS DOUBLE) IS NULL AND a NOT IN ('yes', 'no') ORDER BY timestamp DESC LIMIT 200"
    );
    let (counts, roles, texts) = futures::join!(
        q.sql(project, &counts_sql),
        q.sql(project, &roles_sql),
        q.sql(project, &text_sql)
    );
    let (counts, roles, texts) = (counts?.rows, roles?.rows, texts?.rows);
    let mut out: Vec<SurveyResults> = Vec::new();
    for r in &counts {
        let Some(k) = text(r, "k") else { continue };
        let idx = match out.iter().position(|x| x.key == k) {
            Some(i) => i,
            None => {
                out.push(SurveyResults {
                    key: k.clone(),
                    ..Default::default()
                });
                out.len() - 1
            }
        };
        let n = int(r, "n");
        if int(r, "d") == 1 {
            out[idx].dismissed += n;
        } else if let Some(a) = text(r, "a") {
            out[idx].answers.push((a, n));
        }
    }
    for s in &mut out {
        let rated: Vec<(f64, i64)> = s
            .answers
            .iter()
            .filter_map(|(a, n)| {
                a.parse::<f64>()
                    .ok()
                    .filter(|v| (1.0..=5.0).contains(v))
                    .map(|v| (v, *n))
            })
            .collect();
        let total: i64 = rated.iter().map(|x| x.1).sum();
        if total > 0 {
            s.mean = Some(rated.iter().map(|(v, n)| v * *n as f64).sum::<f64>() / total as f64);
        }
        s.by_role = roles
            .iter()
            .filter(|r| text(r, "k").as_deref() == Some(s.key.as_str()))
            .map(|r| {
                (
                    text(r, "role").unwrap_or_default(),
                    float(r, "m"),
                    int(r, "n"),
                )
            })
            .collect();
        s.latest_text = texts
            .iter()
            .filter(|r| text(r, "k").as_deref() == Some(s.key.as_str()))
            .filter_map(|r| text(r, "a"))
            .take(10)
            .collect();
    }
    Ok(out)
}
