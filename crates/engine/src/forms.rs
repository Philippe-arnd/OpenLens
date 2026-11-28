//! Form analytics (#232): which field makes people give up. From `$field`
//! (a field left, with time spent and whether it changed) and `$submit`
//! events, per form and session: starts, submits, abandonment, time to
//! fill, the last field before giving up, and fields people come back to.
//!
//! Above the daily budget the SDK sends `$field` from a sample of sessions
//! only (#702, D-097), each with the rate as `$sample`: a session counts
//! for `1 / $sample` sessions, so starts, submits and the per-field counts
//! stay absolute numbers. A session without `$field` (not drawn) is not a
//! start, so its `$submit` is not counted either and shares stay unbiased.

use std::collections::{BTreeMap, HashMap};

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, SAMPLE_WEIGHT, source, text};

/// One field of a form.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FieldStats {
    /// Name (or label).
    pub name: String,
    /// Sessions that touched it.
    pub sessions: i64,
    /// Abandoned sessions where it was the last field touched.
    pub last_before_leaving: i64,
    /// Sessions that came back to it after leaving it (a correction).
    pub revisited: i64,
    /// Median time spent in it (ms).
    pub median_ms: i64,
}

/// One form.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FormStats {
    /// `name` or `id` of the form, else the page path.
    pub form: String,
    /// Page it was seen on most.
    pub path: String,
    /// Sessions that started filling it.
    pub starts: i64,
    /// Of them, submitted it.
    pub submits: i64,
    /// Median time to fill, for submitted sessions (seconds).
    pub median_secs: f64,
    /// Fields, the most abandoned first.
    pub fields: Vec<FieldStats>,
    /// A session that abandoned it (for a replay link).
    pub abandoned_session: Option<String>,
}

impl FormStats {
    /// Share of starts that did not submit.
    #[must_use]
    pub fn abandonment(&self) -> f64 {
        1.0 - self.submits as f64 / self.starts.max(1) as f64
    }
}

fn median(v: &mut [i64]) -> i64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// One field visit or submit, in time order within a session.
#[derive(Debug, Clone)]
pub struct Step {
    /// Session.
    pub session: String,
    /// Form key.
    pub form: String,
    /// Field name, None for a submit.
    pub field: Option<String>,
    /// Time in the field (ms).
    pub ms: i64,
    /// Page.
    pub path: String,
    /// Sessions this step stands for: `1 / $sample` of a sampled `$field`,
    /// else 1 (#702).
    pub weight: f64,
}

/// Per form while folding: the stats, each field with its times, the
/// fill times of sent forms, and the pages it was seen on.
type Acc = (
    FormStats,
    HashMap<String, (FieldStats, Vec<i64>)>,
    Vec<i64>,
    HashMap<String, i64>,
);

/// Fold ordered steps into per-form stats. Pure (unit-tested).
#[must_use]
pub fn fold(steps: &[Step]) -> Vec<FormStats> {
    #[derive(Default)]
    struct Visit {
        fields: Vec<(String, i64)>,
        submitted: bool,
        path: String,
        weight: f64,
    }
    // Weighted counts while folding, rounded at the end.
    #[derive(Default)]
    struct Counts {
        starts: f64,
        submits: f64,
        fields: HashMap<String, [f64; 3]>,
    }
    let mut visits: BTreeMap<(String, String), Visit> = BTreeMap::new();
    for s in steps {
        let v = visits
            .entry((s.form.clone(), s.session.clone()))
            .or_default();
        if v.path.is_empty() {
            v.path.clone_from(&s.path);
        }
        match &s.field {
            Some(f) => {
                v.fields.push((f.clone(), s.ms));
                v.weight = v.weight.max(s.weight.max(1.0));
            }
            None => v.submitted = true,
        }
    }
    let mut counts: HashMap<String, Counts> = HashMap::new();
    let mut forms: BTreeMap<String, Acc> = BTreeMap::new();
    for ((form, session), v) in visits {
        if v.fields.is_empty() {
            continue;
        }
        let e = forms.entry(form.clone()).or_insert_with(|| {
            (
                FormStats {
                    form: form.clone(),
                    ..FormStats::default()
                },
                HashMap::new(),
                Vec::new(),
                HashMap::new(),
            )
        });
        let w = v.weight;
        let c = counts.entry(form.clone()).or_default();
        c.starts += w;
        *e.3.entry(v.path.clone()).or_default() += 1;
        if v.submitted {
            c.submits += w;
            e.2.push(v.fields.iter().map(|f| f.1).sum());
        } else if e.0.abandoned_session.is_none() {
            e.0.abandoned_session = Some(session.clone());
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (i, (f, ms)) in v.fields.iter().enumerate() {
            let fs = e.1.entry(f.clone()).or_insert_with(|| {
                (
                    FieldStats {
                        name: f.clone(),
                        ..FieldStats::default()
                    },
                    Vec::new(),
                )
            });
            fs.1.push(*ms);
            let fc = c.fields.entry(f.clone()).or_default();
            match seen.get(f.as_str()) {
                // Back to a field after another one: a correction.
                Some(prev) if *prev + 1 < i => fc[2] += w,
                Some(_) => {}
                None => fc[0] += w,
            }
            seen.insert(f.as_str(), i);
        }
        if !v.submitted
            && let Some((last, _)) = v.fields.last()
        {
            c.fields.entry(last.clone()).or_default()[1] += w;
        }
    }
    let n = |x: f64| x.round() as i64;
    let mut out: Vec<FormStats> = forms
        .into_values()
        .map(|(mut f, fields, mut times, paths)| {
            let c = counts.remove(&f.form).unwrap_or_default();
            (f.starts, f.submits) = (n(c.starts), n(c.submits));
            f.median_secs = median(&mut times) as f64 / 1000.0;
            f.path = paths
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map(|p| p.0)
                .unwrap_or_default();
            let mut fl: Vec<FieldStats> = fields
                .into_values()
                .map(|(mut s, mut ms)| {
                    s.median_ms = median(&mut ms);
                    if let Some(fc) = c.fields.get(&s.name) {
                        (s.sessions, s.last_before_leaving, s.revisited) =
                            (n(fc[0]), n(fc[1]), n(fc[2]));
                    }
                    s
                })
                .collect();
            fl.sort_by(|a, b| {
                b.last_before_leaving
                    .cmp(&a.last_before_leaving)
                    .then(b.sessions.cmp(&a.sessions))
                    .then(a.name.cmp(&b.name))
            });
            f.fields = fl;
            f
        })
        .collect();
    out.sort_by(|a, b| b.starts.cmp(&a.starts).then(a.form.cmp(&b.form)));
    out
}

/// Forms of the last 30 days.
pub async fn forms(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<Vec<FormStats>> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let w = f.clause(30)?;
    let form =
        "coalesce(regexp_match(properties, '\"\\$form\":\"([^\"]{1,100})\"')[1], path, '(page)')";
    let sql = format!(
        "SELECT session_id, {form} AS form, regexp_match(properties, '\"\\$field\":\"([^\"]{{1,60}})\"')[1] AS field, \
         CAST(regexp_match(properties, '\"\\$ms\":([0-9]+)')[1] AS BIGINT) AS ms, event, coalesce(path, '') AS path, \
         {SAMPLE_WEIGHT} AS w \
         FROM {src} WHERE {w} AND event IN ('$field', '$submit') AND session_id IS NOT NULL \
         ORDER BY session_id, timestamp LIMIT 300000"
    );
    let rows = q.sql(project, &sql).await?.rows;
    let steps: Vec<Step> = rows
        .iter()
        .filter_map(|r| {
            let event = text(r, "event")?;
            let field = if event == "$field" {
                Some(text(r, "field")?)
            } else {
                None
            };
            Some(Step {
                session: text(r, "session_id")?,
                form: text(r, "form")?,
                field,
                ms: r.get("ms").and_then(serde_json::Value::as_i64).unwrap_or(0),
                path: text(r, "path").unwrap_or_default(),
                weight: r
                    .get("w")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(1.0),
            })
        })
        .collect();
    Ok(fold(&steps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(session: &str, field: Option<&str>, ms: i64) -> Step {
        Step {
            session: session.into(),
            form: "signup".into(),
            field: field.map(Into::into),
            ms,
            path: "/signup".into(),
            weight: 1.0,
        }
    }

    #[test]
    fn finds_the_field_where_people_give_up() {
        let steps = vec![
            st("a", Some("email"), 2000),
            st("a", Some("password2"), 1000),
            st("a", None, 0),
            st("b", Some("email"), 3000),
            st("b", Some("phone"), 9000),
            st("c", Some("email"), 1000),
            st("c", Some("phone"), 4000),
            st("c", Some("email"), 500),
            st("d", Some("email"), 1000),
            st("d", Some("phone"), 2000),
        ];
        let f = &fold(&steps)[0];
        assert_eq!((f.starts, f.submits), (4, 1));
        assert!((f.abandonment() - 0.75).abs() < 1e-9);
        assert_eq!(f.fields[0].name, "phone", "the top drop-off");
        assert_eq!(f.fields[0].last_before_leaving, 2);
        let email = f
            .fields
            .iter()
            .find(|x| x.name == "email")
            .unwrap_or(&f.fields[0]);
        assert_eq!(
            (email.sessions, email.revisited),
            (4, 1),
            "c came back to email"
        );
        assert!((f.median_secs - 3.0).abs() < 1e-9);
        assert_eq!(f.path, "/signup");
        assert!(f.abandoned_session.is_some());
    }

    /// `$field` sent from a sample of sessions (#702): counts are scaled
    /// back up, shares are unchanged.
    #[test]
    fn sampled_fields_count_for_the_sessions_they_stand_for() {
        let all = vec![
            st("a", Some("email"), 1000),
            st("a", None, 0),
            st("b", Some("email"), 1000),
        ];
        let sampled: Vec<Step> = all
            .iter()
            .map(|s| Step {
                weight: 4.0,
                ..s.clone()
            })
            .collect();
        let (f, g) = (&fold(&all)[0], &fold(&sampled)[0]);
        assert!(g.starts > f.starts && g.submits > f.submits, "{g:?}");
        assert!((g.abandonment() - f.abandonment()).abs() < 1e-9);
        assert!(g.fields[0].sessions > f.fields[0].sessions);
    }
}
