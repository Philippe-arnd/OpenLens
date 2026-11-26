//! Web Vitals explained (#408): one knowledge table (what each vital
//! means to a visitor, its thresholds, its likely causes and what to do,
//! most likely first) and the `vitals` command, which adds the measured
//! p75, the worst pages and, for LCP, the dominant cause.
//!
//! The thresholds are the public ones (web.dev, `docs/clean-room.md`), the
//! same the SDK rates with. The wording is our own. `docs/kpis.md` carries
//! [`markdown`] verbatim, and a test fails when they drift apart.

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, Vital, float, int, text};

/// A likely cause of a poor vital and what fixes it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Cause {
    /// Stable key (`server`, `load`, …).
    pub key: &'static str,
    /// The cause, in plain words.
    pub what: &'static str,
    /// What to do about it, most effective first.
    pub actions: &'static [&'static str],
}

/// One Web Vital in the knowledge table.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct VitalGuide {
    /// Short name: LCP, INP, CLS, FCP or TTFB.
    pub name: &'static str,
    /// Full name.
    pub title: &'static str,
    /// What the visitor feels, in plain words.
    pub feels: &'static str,
    /// `ms`, or empty for the unitless CLS score.
    pub unit: &'static str,
    /// At or below: good.
    pub good: f64,
    /// Above: poor (between the two: needs improvement).
    pub poor: f64,
    /// Whether the board ranks its worst pages and causes (part 1 of #408:
    /// LCP, INP and CLS).
    pub pages: bool,
    /// Likely causes, most likely first.
    pub causes: &'static [Cause],
}

/// The knowledge table, in the order the board lists the vitals.
pub const VITALS_GUIDE: &[VitalGuide] = &[
    VitalGuide {
        name: "LCP",
        title: "Largest Contentful Paint",
        feels: "How long until the main content (the largest image or block of text) is on screen: when the visitor feels the page has loaded.",
        unit: "ms",
        good: 2500.0,
        poor: 4000.0,
        pages: true,
        causes: &[
            Cause {
                key: "load",
                what: "The main image or text block is found late or downloads slowly.",
                actions: &[
                    "Put the main image in the HTML itself (not added by a script), do not lazy-load it, and give it fetchpriority=\"high\".",
                    "Make the main image lighter: a modern format, sized for the screen, served from a CDN with long caching.",
                ],
            },
            Cause {
                key: "render",
                what: "Styles and scripts keep the browser from drawing the page.",
                actions: &[
                    "Inline the styles the first screen needs and defer the scripts it does not need.",
                    "Render the first screen on the server instead of in the browser.",
                ],
            },
            Cause {
                key: "server",
                what: "The server is slow to send the first byte of the page.",
                actions: &[
                    "Cache the page's HTML (or put a CDN in front of it), remove redirects, and speed up the server work behind the page.",
                ],
            },
        ],
    },
    VitalGuide {
        name: "INP",
        title: "Interaction to Next Paint",
        feels: "How long the page takes to show a response after a click, tap or key press, for about the slowest interaction of the visit: lag.",
        unit: "ms",
        good: 200.0,
        poor: 500.0,
        pages: true,
        causes: &[
            Cause {
                key: "busy",
                what: "The page is busy running scripts when the visitor acts.",
                actions: &[
                    "Split long scripts into small tasks that hand control back to the browser, and load third-party scripts later.",
                ],
            },
            Cause {
                key: "handlers",
                what: "The code that answers a click or key press does too much at once.",
                actions: &[
                    "Make the visible change first and move the rest of the work after the next paint (or to a worker).",
                ],
            },
            Cause {
                key: "render",
                what: "Drawing the next frame takes long.",
                actions: &[
                    "Keep the page's DOM small, do not read layout right after changing styles, and skip rendering off-screen content (content-visibility).",
                ],
            },
        ],
    },
    VitalGuide {
        name: "CLS",
        title: "Cumulative Layout Shift",
        feels: "How much the content jumps while the visitor reads, over the worst burst of shifts: the reason people click the wrong thing.",
        unit: "",
        good: 0.1,
        poor: 0.25,
        pages: true,
        causes: &[
            Cause {
                key: "media",
                what: "Images, videos, ads or embeds arrive without space kept for them.",
                actions: &[
                    "Give every image, video and iframe a width and height (or a CSS aspect-ratio), and keep space for ads and embeds.",
                ],
            },
            Cause {
                key: "injected",
                what: "Content is inserted above what the visitor is reading (banners, late results).",
                actions: &[
                    "Keep the space before the content arrives, or insert it below the visible part or only after a visitor action.",
                ],
            },
            Cause {
                key: "fonts",
                what: "Web fonts swap in and re-flow the text.",
                actions: &[
                    "Preload the main fonts and pick a fallback font of the same size (or font-display: optional).",
                ],
            },
            Cause {
                key: "animation",
                what: "Animations move elements by changing their position or size.",
                actions: &["Animate with transform and opacity only."],
            },
        ],
    },
    VitalGuide {
        name: "FCP",
        title: "First Contentful Paint",
        feels: "How long until anything at all (text, an image, a background) appears: the time the visitor stares at a blank screen.",
        unit: "ms",
        good: 1800.0,
        poor: 3000.0,
        pages: false,
        causes: &[
            Cause {
                key: "blocking",
                what: "Styles and scripts in the page head block the first paint.",
                actions: &[
                    "Inline the critical styles, defer or async the scripts, and drop the CSS the page does not use.",
                ],
            },
            Cause {
                key: "server",
                what: "The first byte of the page arrives late.",
                actions: &["Speed up the server response first (see TTFB)."],
            },
            Cause {
                key: "fonts",
                what: "Text stays invisible until the web fonts load.",
                actions: &[
                    "Preload the main fonts and show a fallback font meanwhile (font-display: swap).",
                ],
            },
        ],
    },
    VitalGuide {
        name: "TTFB",
        title: "Time to First Byte",
        feels: "How long until the server starts answering the page request: everything else on the page waits for it.",
        unit: "ms",
        good: 800.0,
        poor: 1800.0,
        pages: false,
        causes: &[
            Cause {
                key: "server",
                what: "The server does slow work (database, rendering) before it answers.",
                actions: &[
                    "Cache the page's HTML or the slow queries behind it, and measure where the server time goes.",
                ],
            },
            Cause {
                key: "distance",
                what: "Visitors are far from the server.",
                actions: &["Serve the pages from a CDN or a region closer to the visitors."],
            },
            Cause {
                key: "redirects",
                what: "Redirects run before the page.",
                actions: &[
                    "Link straight to the final address (no http to https or trailing-slash hops).",
                ],
            },
        ],
    },
];

/// Worst pages listed per vital.
pub const WORST_PAGES: usize = 5;

/// From this share of LCP spent waiting for the first byte (on the same
/// pages), the server is the dominant cause: more than half of the wait.
pub const SERVER_SHARE: f64 = 0.5;

/// The entry of the knowledge table for a vital.
pub fn vitals_guide(name: &str) -> Option<&'static VitalGuide> {
    VITALS_GUIDE.iter().find(|g| g.name == name)
}

/// good / needs-improvement / poor, or `unknown` for a name not in the
/// table.
pub fn rating(name: &str, v: f64) -> &'static str {
    let Some(g) = vitals_guide(name) else {
        return "unknown";
    };
    if v <= g.good {
        "good"
    } else if v <= g.poor {
        "needs-improvement"
    } else {
        "poor"
    }
}

/// A vital's value for people: `2.5 s`, `180 ms`, or `0.12` for CLS.
pub fn show(name: &str, v: f64) -> String {
    match vitals_guide(name).map(|g| g.unit) {
        Some("") => format!("{v:.2}"),
        _ if v >= 1000.0 => format!("{:.1} s", v / 1000.0),
        _ => format!("{v:.0} ms"),
    }
}

/// One page's p75 for one vital.
#[derive(Debug, Clone, Serialize)]
pub struct PageVital {
    /// LCP, INP, CLS or TTFB.
    pub name: String,
    /// The event's path.
    pub path: String,
    /// p75 on this page.
    pub p75: f64,
    /// Measurements on this page.
    pub samples: i64,
    /// good / needs-improvement / poor.
    pub rating: &'static str,
}

/// The cause that weighs most, when the data shows one.
#[derive(Debug, Clone, Serialize)]
pub struct Dominant {
    /// Key of the cause in the table.
    pub cause: &'static str,
    /// The cause, in plain words.
    pub what: &'static str,
    /// Mean share of LCP spent waiting for the first byte, 0-1.
    pub ttfb_share: f64,
    /// Pages where both LCP and TTFB were measured.
    pub pages: usize,
}

/// A vital explained: the table entry, the measure and what to do.
#[derive(Debug, Clone, Serialize)]
pub struct VitalReport {
    /// LCP, INP, CLS, FCP or TTFB.
    pub name: &'static str,
    /// Full name.
    pub title: &'static str,
    /// What the visitor feels.
    pub feels: &'static str,
    /// `ms`, or empty for CLS.
    pub unit: &'static str,
    /// Good at or below.
    pub good: f64,
    /// Poor above.
    pub poor: f64,
    /// p75 over the last 7 days, when measured.
    pub p75: Option<f64>,
    /// good / needs-improvement / poor, or `not measured`.
    pub rating: &'static str,
    /// Measurements behind the p75.
    pub samples: i64,
    /// Worst pages at p75 (LCP, INP, CLS; pages with enough measurements).
    pub worst_pages: Vec<PageVital>,
    /// The dominant cause (LCP, when TTFB is measured on its worst pages).
    pub dominant_cause: Option<Dominant>,
    /// Likely causes, most likely first (the dominant one leads).
    pub causes: Vec<&'static str>,
    /// What to do, in the order of the causes.
    pub actions: Vec<&'static str>,
}

impl VitalReport {
    /// The first thing to do.
    pub fn first_action(&self) -> Option<&'static str> {
        self.actions.first().copied()
    }
}

/// The values of every `$vital` over the last 7 days: `name`, `v`, `path`.
fn values(src: &str, w7: &str) -> String {
    format!(
        "SELECT regexp_match(properties, '\"\\$name\":\"([A-Z]+)\"')[1] AS name, \
                CAST(regexp_match(properties, '\"\\$value\":([0-9.eE+-]+)')[1] AS DOUBLE) AS v, path \
         FROM {src} WHERE {w7} AND event = '$vital'"
    )
}

/// p75 and samples per vital (the board's Web Vitals card).
pub(crate) fn p75_sql(src: &str, w7: &str) -> String {
    format!(
        "SELECT name, approx_percentile_cont(v, 0.75) AS p75, count(*) AS samples FROM ({}) x \
         WHERE name IS NOT NULL AND v IS NOT NULL GROUP BY name ORDER BY name",
        values(src, w7)
    )
}

/// The worst pages at p75 of LCP, INP and CLS (at most [`WORST_PAGES`]
/// each, with enough measurements), plus TTFB on LCP's worst pages.
fn pages_sql(src: &str, w7: &str) -> String {
    let min = crate::insights::MIN_GROUP;
    format!(
        "WITH g AS (SELECT name, path, approx_percentile_cont(v, 0.75) AS p75, count(*) AS samples \
           FROM ({}) x WHERE name IN ('LCP', 'INP', 'CLS', 'TTFB') AND v IS NOT NULL AND path IS NOT NULL \
           GROUP BY name, path HAVING count(*) >= {min}) \
         SELECT * FROM (SELECT name, path, p75, samples FROM g WHERE name = 'LCP' ORDER BY p75 DESC, path LIMIT {WORST_PAGES}) l \
         UNION ALL SELECT * FROM (SELECT name, path, p75, samples FROM g WHERE name = 'INP' ORDER BY p75 DESC, path LIMIT {WORST_PAGES}) i \
         UNION ALL SELECT * FROM (SELECT name, path, p75, samples FROM g WHERE name = 'CLS' ORDER BY p75 DESC, path LIMIT {WORST_PAGES}) c \
         UNION ALL SELECT name, path, p75, samples FROM g WHERE name = 'TTFB' AND path IN \
           (SELECT path FROM g WHERE name = 'LCP' ORDER BY p75 DESC, path LIMIT {WORST_PAGES})",
        values(src, w7)
    )
}

/// The rows of [`pages_sql`], over a source already filtered by `f`'s
/// parameters.
pub(crate) async fn pages(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<PageVital>> {
    let rows = q.sql(project, &pages_sql(src, &f.clause(7)?)).await?.rows;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let name = text(r, "name")?;
            let p75 = float(r, "p75");
            Some(PageVital {
                rating: rating(&name, p75),
                path: text(r, "path")?,
                samples: int(r, "samples"),
                name,
                p75,
            })
        })
        .collect())
}

/// LCP's dominant cause, from the share of TTFB in LCP on the pages where
/// both were measured.
fn dominant(g: &VitalGuide, pages: &[PageVital]) -> Option<Dominant> {
    if g.name != "LCP" {
        return None;
    }
    // The pages that make LCP slow decide; a fast page with a relatively
    // large TTFB share must not dilute them. Without such a page, all count.
    let lcp: Vec<&PageVital> = pages
        .iter()
        .filter(|p| p.name == "LCP" && p.p75 > 0.0)
        .collect();
    let slow: Vec<&PageVital> = lcp.iter().copied().filter(|p| p.rating != "good").collect();
    let shares: Vec<f64> = if slow.is_empty() { lcp } else { slow }
        .into_iter()
        .filter_map(|lcp| {
            let ttfb = pages
                .iter()
                .find(|t| t.name == "TTFB" && t.path == lcp.path)?;
            Some((ttfb.p75 / lcp.p75).clamp(0.0, 1.0))
        })
        .collect();
    if shares.is_empty() {
        return None;
    }
    let share = shares.iter().sum::<f64>() / shares.len() as f64;
    let key = if share >= SERVER_SHARE {
        "server"
    } else {
        "load"
    };
    let cause = g.causes.iter().find(|c| c.key == key)?;
    Some(Dominant {
        cause: cause.key,
        what: cause.what,
        ttfb_share: share,
        pages: shares.len(),
    })
}

/// Explain the vitals: every vital of the table, or with `not_good` only
/// the measured ones rated needs-improvement or poor.
pub fn explain(vitals: &[Vital], pages: &[PageVital], not_good: bool) -> Vec<VitalReport> {
    VITALS_GUIDE
        .iter()
        .filter_map(|g| {
            let measured = vitals.iter().find(|v| v.name == g.name);
            let rating = measured.map_or("not measured", |v| rating(g.name, v.p75));
            if not_good && matches!(rating, "good" | "not measured") {
                return None;
            }
            let mut worst: Vec<PageVital> = if g.pages {
                pages.iter().filter(|p| p.name == g.name).cloned().collect()
            } else {
                Vec::new()
            };
            worst.sort_by(|a, b| b.p75.total_cmp(&a.p75).then_with(|| a.path.cmp(&b.path)));
            worst.truncate(WORST_PAGES);
            let dominant = dominant(g, pages);
            let mut causes: Vec<&Cause> = g.causes.iter().collect();
            if let Some(d) = &dominant {
                causes.sort_by_key(|c| c.key != d.cause);
            }
            Some(VitalReport {
                name: g.name,
                title: g.title,
                feels: g.feels,
                unit: g.unit,
                good: g.good,
                poor: g.poor,
                p75: measured.map(|v| v.p75),
                rating,
                samples: measured.map_or(0, |v| v.samples),
                worst_pages: worst,
                dominant_cause: dominant,
                actions: causes
                    .iter()
                    .flat_map(|c| c.actions.iter().copied())
                    .collect(),
                causes: causes.iter().map(|c| c.what).collect(),
            })
        })
        .collect()
}

/// The `vitals` command: every Web Vital of the last 7 days with its p75,
/// rating, thresholds, worst pages and ranked actions.
pub async fn vitals(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
) -> Result<Vec<VitalReport>> {
    let q = &q.with_params(&f.params()?);
    let src = crate::source(map);
    let w7 = f.clause(7)?;
    let p75 = p75_sql(&src, &w7);
    let (overall, pages) = futures::join!(q.sql(project, &p75), pages(q, project, &src, f));
    let measured: Vec<Vital> = overall?
        .rows
        .iter()
        .filter_map(|r| {
            let name = text(r, "name")?;
            let p75 = float(r, "p75");
            Some(Vital {
                rating: rating(&name, p75).into(),
                name,
                p75,
                samples: int(r, "samples"),
            })
        })
        .collect();
    Ok(explain(&measured, &pages?, false))
}

/// The Web Vitals section of `docs/kpis.md`, generated from the table.
pub fn markdown() -> String {
    let mut s = String::from(
        "### Web Vitals explained\n\n\
         Generated from `crates/engine/src/vitals.rs`; a test fails when this section and the table differ. \
         The board, Ask and the `vitals` MCP tool all read the same table.\n\n\
         | Vital | What the visitor feels | Good | Poor |\n| --- | --- | --- | --- |\n",
    );
    for g in VITALS_GUIDE {
        s.push_str(&format!(
            "| **{}** ({}) | {} | ≤ {} | > {} |\n",
            g.name,
            g.title,
            g.feels,
            show(g.name, g.good),
            show(g.name, g.poor)
        ));
    }
    s.push_str(&format!(
        "\nA vital that is not good opens *Why and what to do* on the board: for LCP, INP and CLS, the worst pages at p75 \
         (pages with at least {} measurements in the last 7 days), then the causes and actions below, most likely first. \
         For LCP, when TTFB is measured on the same pages and is more than half of LCP, the server cause comes first.\n",
        crate::insights::MIN_GROUP
    ));
    for g in VITALS_GUIDE {
        s.push_str(&format!(
            "\n**{}: likely causes and what to do**\n\n",
            g.name
        ));
        for (i, c) in g.causes.iter().enumerate() {
            s.push_str(&format!("{}. {} {}\n", i + 1, c.what, c.actions.join(" ")));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn vital(name: &str, p75: f64) -> Vital {
        Vital {
            name: name.into(),
            p75,
            rating: rating(name, p75).into(),
            samples: 50,
        }
    }

    fn page(name: &str, path: &str, p75: f64) -> PageVital {
        PageVital {
            name: name.into(),
            path: path.into(),
            p75,
            samples: 20,
            rating: rating(name, p75),
        }
    }

    /// The doc carries the generated section verbatim.
    #[test]
    fn kpis_doc_matches_the_table() {
        let doc = include_str!("../../../docs/kpis.md");
        assert!(
            doc.contains(&markdown()),
            "docs/kpis.md's Web Vitals section drifted from vitals.rs: paste markdown() in"
        );
    }

    /// The SDK rates with the same thresholds as the engine.
    #[test]
    fn sdk_thresholds_match_the_table() {
        let sdk = include_str!("../../../sdk/js/src/vitals.ts");
        for g in VITALS_GUIDE {
            let line = format!("{}: [{}, {}]", g.name, g.good, g.poor);
            assert!(sdk.contains(&line), "SDK thresholds differ for {line}");
        }
    }

    #[test]
    fn server_leads_when_the_first_byte_is_most_of_lcp() {
        let v = [vital("LCP", 6000.0)];
        let slow_server = [page("LCP", "/a", 6000.0), page("TTFB", "/a", 4000.0)];
        let slow_image = [page("LCP", "/a", 6000.0), page("TTFB", "/a", 500.0)];
        let server = &explain(&v, &slow_server, true)[0];
        let image = &explain(&v, &slow_image, true)[0];
        assert_eq!(server.dominant_cause.as_ref().unwrap().cause, "server");
        assert_ne!(server.first_action(), image.first_action());
        assert_eq!(
            image.dominant_cause.as_ref().unwrap().cause,
            "load",
            "the wait is after the first byte"
        );
        // Without TTFB on the same pages, no cause is claimed.
        let alone = &explain(&v, &[page("LCP", "/a", 6000.0)], true)[0];
        assert!(alone.dominant_cause.is_none());
    }

    #[test]
    fn only_vitals_that_are_not_good_are_explained_on_the_board() {
        let v = [vital("LCP", 1000.0), vital("CLS", 0.4)];
        let why = explain(&v, &[], true);
        assert!(why.iter().all(|r| r.name == "CLS"), "{why:?}");
        assert!(explain(&v, &[], false).len() > why.len());
    }

    #[test]
    fn worst_pages_come_first_and_are_bounded() {
        let v = [vital("INP", 900.0)];
        let mut pages: Vec<PageVital> = (0..9)
            .map(|i| page("INP", &format!("/p{i}"), 100.0 * f64::from(i)))
            .collect();
        pages.push(page("LCP", "/x", 9999.0));
        let r = &explain(&v, &pages, true)[0];
        assert!(r.worst_pages.len() <= WORST_PAGES);
        assert!(r.worst_pages.iter().all(|p| p.name == "INP"));
        assert!(r.worst_pages[0].p75 > r.worst_pages[1].p75);
    }
}
