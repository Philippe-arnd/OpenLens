//! Where a slow Web Vital comes from (#467): what the SDK's attribution
//! says about the slow measurements of the worst pages. The guilty
//! elements (selectors), the part of the vital that weighs most (LCP's four
//! parts, INP's three) and the p75 per device class.
//!
//! Events of an older SDK carry none of it: every list is then empty and
//! [`crate::vitals`] explains the vital as before.

use openlens_query::QueryEngine;
use serde::Serialize;

use crate::vitals::{VITALS_GUIDE, WORST_PAGES, rating};
use crate::{Filters, Result, float, int, text};

/// A part of a vital, as the SDK sends it, and the cause it points to.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Phase {
    /// LCP or INP.
    pub vital: &'static str,
    /// The `$vital` property, without `$`.
    pub key: &'static str,
    /// In plain words.
    pub label: &'static str,
    /// Key of the cause in the knowledge table.
    pub cause: &'static str,
}

/// The parts of LCP and INP (web.dev, `docs/clean-room.md`).
pub const PHASES: &[Phase] = &[
    Phase {
        vital: "LCP",
        key: "ttfb",
        label: "waiting for the first byte",
        cause: "server",
    },
    Phase {
        vital: "LCP",
        key: "load_delay",
        label: "load delay (the element's resource starts downloading late)",
        cause: "load",
    },
    Phase {
        vital: "LCP",
        key: "load_time",
        label: "load time (the element's resource downloads)",
        cause: "heavy",
    },
    Phase {
        vital: "LCP",
        key: "render_delay",
        label: "render delay (loaded, not yet drawn)",
        cause: "render",
    },
    Phase {
        vital: "INP",
        key: "input_delay",
        label: "input delay (the page is busy when the visitor acts)",
        cause: "busy",
    },
    Phase {
        vital: "INP",
        key: "processing",
        label: "processing (the event handlers run)",
        cause: "handlers",
    },
    Phase {
        vital: "INP",
        key: "presentation",
        label: "presentation (drawing the next frame)",
        cause: "render",
    },
];

/// Elements listed per vital.
pub const TOP_ELEMENTS: usize = 5;

/// An element behind slow measurements of a vital on one of its worst pages.
#[derive(Debug, Clone, Serialize)]
pub struct Culprit {
    /// The page's path.
    pub path: String,
    /// A short CSS selector (no text).
    pub element: String,
    /// LCP: image, text, video or background; INP: pointer or keyboard.
    pub kind: Option<String>,
    /// LCP: the resource's path (no query string).
    pub resource: Option<String>,
    /// Slow measurements naming it.
    pub samples: i64,
}

/// A vital's p75 on one device class.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceVital {
    /// LCP, INP or CLS.
    pub name: String,
    /// phone, tablet or desktop.
    pub device: String,
    /// p75 on this device class.
    pub p75: f64,
    /// Measurements.
    pub samples: i64,
    /// good / needs-improvement / poor.
    pub rating: &'static str,
}

/// The part that weighs most in a vital's slow measurements.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseShare {
    /// The `$vital` property (`load_delay`, …).
    pub key: &'static str,
    /// In plain words.
    pub label: &'static str,
    /// Share of the vital, 0-1.
    pub share: f64,
    /// Slow measurements that carried the parts.
    pub samples: i64,
}

/// The sum of each part of a vital, by `$vital` property.
pub type PartSums = Vec<(&'static str, f64)>;

/// What the attribution says, for every vital.
#[derive(Debug, Clone, Default)]
pub struct Attribution {
    /// Elements, most frequent first, per vital.
    pub culprits: Vec<(String, Culprit)>,
    /// Per vital: slow measurements with parts and the sum of each part.
    pub parts: Vec<(String, i64, PartSums)>,
    /// p75 per device class.
    pub devices: Vec<DeviceVital>,
}

impl Attribution {
    /// The top elements of a vital, most frequent first.
    pub fn elements(&self, vital: &str) -> Vec<Culprit> {
        let mut v: Vec<Culprit> = self
            .culprits
            .iter()
            .filter(|(n, _)| n == vital)
            .map(|(_, c)| c.clone())
            .collect();
        v.sort_by(|a, b| {
            b.samples
                .cmp(&a.samples)
                .then_with(|| a.element.cmp(&b.element))
        });
        v.truncate(TOP_ELEMENTS);
        v
    }

    /// The part that weighs most, with enough measurements to say.
    pub fn dominant_phase(&self, vital: &str) -> Option<(&'static Phase, PhaseShare)> {
        let (_, n, sums) = self.parts.iter().find(|(v, n, _)| v == vital && *n > 0)?;
        if *n < crate::insights::MIN_GROUP {
            return None;
        }
        let total: f64 = sums.iter().map(|(_, s)| s.max(0.0)).sum();
        let (key, top) = sums.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        if total <= 0.0 {
            return None;
        }
        let phase = PHASES.iter().find(|p| p.vital == vital && p.key == *key)?;
        Some((
            phase,
            PhaseShare {
                key: phase.key,
                label: phase.label,
                share: top / total,
                samples: *n,
            },
        ))
    }

    /// p75 per device class of a vital, slowest first.
    pub fn devices(&self, vital: &str) -> Vec<DeviceVital> {
        let mut v: Vec<DeviceVital> = self
            .devices
            .iter()
            .filter(|d| d.name == vital)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.p75.total_cmp(&a.p75));
        v
    }
}

/// `$name`, `$value`, `$device`, the element, its kind and resource and
/// every part, for each `$vital` of the last 7 days, from the typed
/// columns (#449). Their values are bounded at write time as the patterns
/// that read them before: a hostile client cannot make a group key longer
/// than the SDK's caps.
fn facts(src: &str, w7: &str) -> String {
    let parts: String = PHASES
        .iter()
        .map(|p| format!(", vital_{k} AS {k}", k = p.key))
        .collect();
    format!(
        "SELECT vital_name AS name, vital_value AS v, path, vital_device AS device, \
                vital_el AS el, vital_kind AS kind, vital_res AS res{parts} \
         FROM {src} WHERE {w7} AND event = '$vital'"
    )
}

/// The slow measurements (rated worse than good) of LCP, INP and CLS on
/// their worst pages (the same pages as `vitals::pages`), then per vital
/// its most frequent elements (bounded) and the sum of each part.
fn attribution_sql(src: &str, w7: &str) -> String {
    let min = crate::insights::MIN_GROUP;
    let slow: Vec<String> = ["LCP", "INP", "CLS"]
        .iter()
        .filter_map(|n| VITALS_GUIDE.iter().find(|g| g.name == *n))
        .map(|g| {
            format!(
                "SELECT * FROM a WHERE name = '{n}' AND v > {good} AND path IN \
                 (SELECT path FROM g WHERE name = '{n}' ORDER BY p75 DESC, path LIMIT {WORST_PAGES})",
                n = g.name,
                good = g.good
            )
        })
        .collect();
    let nulls: String = PHASES
        .iter()
        .map(|p| format!(", CAST(NULL AS DOUBLE) AS {}", p.key))
        .collect();
    let sums: String = PHASES
        .iter()
        .map(|p| format!(", sum({k}) AS {k}", k = p.key))
        .collect();
    let limit = TOP_ELEMENTS * WORST_PAGES * 3;
    format!(
        "WITH a AS ({facts}), \
         g AS (SELECT name, path, approx_percentile_cont(v, 0.75) AS p75 FROM a \
               WHERE name IN ('LCP', 'INP', 'CLS') AND v IS NOT NULL AND path IS NOT NULL \
               GROUP BY name, path HAVING count(*) >= {min}), \
         s AS ({slow}) \
         SELECT * FROM (SELECT name, path, el, kind, res, count(*) AS n, CAST(0 AS BIGINT) AS timed{nulls} \
           FROM s WHERE el IS NOT NULL GROUP BY name, path, el, kind, res ORDER BY n DESC, el LIMIT {limit}) e \
         UNION ALL SELECT name, CAST(NULL AS VARCHAR) AS path, CAST(NULL AS VARCHAR) AS el, \
           CAST(NULL AS VARCHAR) AS kind, CAST(NULL AS VARCHAR) AS res, count(*) AS n, \
           count(ttfb) + count(input_delay) AS timed{sums} FROM s GROUP BY name",
        facts = facts(src, w7),
        slow = slow.join(" UNION ALL "),
    )
}

/// p75 of LCP, INP and CLS per device class (three classes at most).
fn devices_sql(src: &str, w7: &str) -> String {
    let min = crate::insights::MIN_GROUP;
    format!(
        "SELECT name, device, approx_percentile_cont(v, 0.75) AS p75, count(*) AS samples FROM ({}) x \
         WHERE name IN ('LCP', 'INP', 'CLS') AND v IS NOT NULL AND device IS NOT NULL \
         GROUP BY name, device HAVING count(*) >= {min}",
        facts(src, w7)
    )
}

/// Read the attribution over a source already filtered by `f`'s parameters.
pub(crate) async fn attribution(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Attribution> {
    let w7 = f.clause(7)?;
    let (attr_sql, dev_sql) = (attribution_sql(src, &w7), devices_sql(src, &w7));
    let (attr, dev) = futures::join!(q.sql(project, &attr_sql), q.sql(project, &dev_sql));
    let mut out = Attribution::default();
    for r in &attr?.rows {
        let Some(name) = text(r, "name") else {
            continue;
        };
        match (text(r, "path"), text(r, "el")) {
            (Some(path), Some(element)) => out.culprits.push((
                name,
                Culprit {
                    path,
                    element,
                    kind: text(r, "kind"),
                    resource: text(r, "res"),
                    samples: int(r, "n"),
                },
            )),
            _ => {
                let sums = PHASES
                    .iter()
                    .filter(|p| p.vital == name)
                    .map(|p| (p.key, float(r, p.key)))
                    .collect();
                out.parts.push((name, int(r, "timed"), sums));
            }
        }
    }
    out.devices = dev?
        .rows
        .iter()
        .filter_map(|r| {
            let name = text(r, "name")?;
            let p75 = float(r, "p75");
            Some(DeviceVital {
                rating: rating(&name, p75),
                device: text(r, "device")?,
                samples: int(r, "samples"),
                name,
                p75,
            })
        })
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every part points to a cause the knowledge table has.
    #[test]
    fn every_phase_points_to_a_cause_of_its_vital() {
        for p in PHASES {
            let g = crate::vitals_guide(p.vital);
            assert!(
                g.is_some_and(|g| g.causes.iter().any(|c| c.key == p.cause)),
                "{} {} points to an unknown cause {}",
                p.vital,
                p.key,
                p.cause
            );
        }
    }

    #[test]
    fn the_heaviest_part_wins_once_measured_enough() {
        let sums = |d: f64| {
            vec![
                ("ttfb", 300.0),
                ("load_delay", d),
                ("load_time", 200.0),
                ("render_delay", 100.0),
            ]
        };
        let mut a = Attribution {
            parts: vec![("LCP".into(), 20, sums(3000.0))],
            ..Attribution::default()
        };
        let (p, share) = a
            .dominant_phase("LCP")
            .map(|(p, s)| (p.cause, s.share))
            .unwrap_or_default();
        assert_eq!(p, "load");
        assert!(share > 0.5);
        a.parts = vec![("LCP".into(), 2, sums(3000.0))];
        assert!(a.dominant_phase("LCP").is_none(), "too few measurements");
        assert!(a.dominant_phase("INP").is_none());
    }
}
