//! Where the board's people blocks read from (#492, D-040): users (DAU,
//! WAU, MAU), known, anonymous, roles, role × feature, features and the
//! daily active series count people per day, so a closed day can come
//! from its `person_day` rows (D-038) instead of its events.
//!
//! Each block reads one row per person, environment, release, role,
//! feature and UTC day: `person_day` for a day that is closed, entirely
//! inside the block's window, rolled up, and rolled with the feature map
//! in force; the events, grouped the same way, for every other day
//! (today, yesterday, the day the window starts in, a day not rolled up
//! yet, or one rolled with another map). Days never overlap, so a person
//! is counted once.

use chrono::{DateTime, NaiveDate, Offset, TimeZone, Utc};

use super::*;

/// The board's people rows over a window: which days come from the
/// rollup, and how to read the rest from the events.
pub(crate) struct People<'a> {
    /// The board's event source: the events, with the map's roles and features.
    pub src: &'a str,
    /// The canonical role of a raw `role`: the map's lookup, or `role`.
    pub role: String,
    /// Whether an event is a completed task, on the board's source (the
    /// rule `person_day` counts `successes` with, D-038).
    pub task: &'a str,
    /// Closed days whose `person_day` rows were rolled with the map in
    /// force, oldest first.
    pub rolled: Vec<NaiveDate>,
    /// The board's clock: windows and days are cut at this instant.
    pub now: DateTime<Utc>,
}

impl People<'_> {
    /// A table of people rows for the filters `f` over the last `days`
    /// days: `distinct_id`, `environment`, `release`, `role` (canonical),
    /// `feature`, `events`, `successes` (completed tasks), `first_ts` and
    /// `last_ts` (the first and last event of the row) and `day` (the UTC
    /// day). A person active since a time inside the window
    /// is one with a row whose `last_ts` is at or after it.
    pub(crate) fn rows(&self, f: &Filters, days: i64) -> Result<String> {
        self.rows_in(f, Span::days(days))
    }

    /// [`People::rows`] over any span of time (D-055): the release window
    /// and the same length before it.
    pub(crate) fn rows_in(&self, f: &Filters, span: Span) -> Result<String> {
        let (skip, rolled) = split_span(&self.rolled, self.now, span, f, &self.role);
        let events = format!(
            "SELECT distinct_id, environment, release, role, feature, count(*) AS events, \
             sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS successes, \
             min(timestamp) AS first_ts, max(timestamp) AS last_ts, date AS day \
             FROM {src} WHERE {w}{skip} \
             GROUP BY distinct_id, environment, release, role, feature, date",
            task = self.task,
            src = self.src,
            w = f.clause_span(span, self.now)?,
        );
        let Some(w) = rolled else {
            return Ok(format!("({events}) AS people"));
        };
        let role = &self.role;
        Ok(format!(
            "({events} UNION ALL SELECT person AS distinct_id, environment, release, \
             {role} AS role, feature, events, successes, first_ts, last_ts, date AS day \
             FROM person_day WHERE {w}) AS people"
        ))
    }
}

/// The closed days a per-day rollup `table` (`person_day`, `session_day`)
/// can give for `project` over the last 90 days: rolled up (D-025) with
/// the map version `version` (none on both sides without a map). A day
/// rolled with another map, or not rolled up, is absent and is read from
/// its events (#489).
pub(crate) async fn rolled_days(
    q: &QueryEngine,
    project: &str,
    table: &'static str,
    version: Option<i64>,
    now: DateTime<Utc>,
) -> Result<Vec<NaiveDate>> {
    rolled_days_back(q, project, table, version, now, 90).await
}

/// [`rolled_days`] over the last `back` days: the release window and its
/// comparison reach up to 180 days back (D-055).
pub(crate) async fn rolled_days_back(
    q: &QueryEngine,
    project: &str,
    table: &'static str,
    version: Option<i64>,
    now: DateTime<Utc>,
    back: i64,
) -> Result<Vec<NaiveDate>> {
    let last = openlens_query::last_closed(now);
    let since = (now - Duration::days(back)).date_naive();
    let sql = format!(
        "SELECT date, count(*) AS n, count(map_version) AS versioned, \
         min(map_version) AS lo, max(map_version) AS hi FROM {table} \
         WHERE date > '{since}' AND date <= '{last}' GROUP BY date"
    );
    let mut days: Vec<NaiveDate> = q
        .sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter(|r| match version {
            None => int(r, "versioned") == 0,
            Some(v) => int(r, "versioned") == int(r, "n") && int(r, "lo") == v && int(r, "hi") == v,
        })
        .filter_map(|r| {
            let d = NaiveDate::parse_from_str(&text(r, "date")?, "%Y-%m-%d").ok()?;
            (d > since && d <= last).then_some(d)
        })
        .collect();
    days.sort_unstable();
    Ok(days)
}

/// How a window of `days` days at `now`, with the filters `f`, splits
/// between the `rolled` days and the events: the condition that keeps the
/// events to the days not read from the rollup (empty when every day is
/// read from the events), and the rollup's WHERE clause for the rolled
/// days entirely inside the window (none when no day qualifies). `role`
/// maps the rollup's raw role to the canonical one.
pub(crate) fn split(
    rolled: &[NaiveDate],
    now: DateTime<Utc>,
    days: i64,
    f: &Filters,
    role: &str,
) -> (String, Option<String>) {
    split_span(rolled, now, Span::days(days), f, role)
}

/// [`split`] for any span of time (D-055): a rolled day is read from the
/// rollup when it lies entirely inside the span, after the day the span
/// starts in and before the day it ends in.
///
/// The events side names the days it reads (`date IN (…)`), never the
/// ones it skips: the scan then lists only those days' partitions,
/// whatever the plan makes of the rest of the clause (D-062).
pub(crate) fn split_span(
    rolled: &[NaiveDate],
    now: DateTime<Utc>,
    span: Span,
    f: &Filters,
    role: &str,
) -> (String, Option<String>) {
    let cutoff = (now - Duration::seconds(span.start)).date_naive();
    let until = (now - Duration::seconds(span.end)).date_naive();
    let inside: std::collections::BTreeSet<NaiveDate> = rolled
        .iter()
        .copied()
        .filter(|d| *d > cutoff && *d < until)
        .collect();
    if inside.is_empty() {
        return (String::new(), None);
    }
    let quoted = |d: &NaiveDate| format!("'{}'", d.format("%Y-%m-%d"));
    let list = inside.iter().map(quoted).collect::<Vec<_>>().join(", ");
    // Every other day of the span: its first and last days at least.
    let open = cutoff
        .iter_days()
        .take_while(|d| *d <= until)
        .filter(|d| !inside.contains(d))
        .map(|d| quoted(&d))
        .collect::<Vec<_>>()
        .join(", ");
    // The same filters as `Filters::clause`, on the rollup's columns.
    let mut w = format!("date IN ({list}) AND environment = $env");
    if f.role.is_some() {
        w.push_str(&format!(" AND {role} = $role"));
    }
    if f.release.is_some() {
        w.push_str(" AND release = $release");
    }
    if !f.internal {
        w.push_str(" AND NOT is_internal(person)");
    }
    (format!(" AND date IN ({open})"), Some(w))
}

/// Whether every day from `from` to `to` starts and ends on a UTC
/// midnight in the zone `tz`, so its days are the UTC days of the rollup
/// (D-040). A zone with an offset, or a day with a change of offset, is
/// not: a block that groups by day then stays on the events.
pub(crate) fn utc_days(tz: &str, from: NaiveDate, to: NaiveDate) -> bool {
    if tz == "UTC" {
        return true;
    }
    let Ok(zone) = tz.parse::<chrono_tz::Tz>() else {
        return false;
    };
    from.iter_days()
        .take_while(|d| *d <= to.succ_opt().unwrap_or(to))
        .all(|d| {
            let midnight = d.and_time(chrono::NaiveTime::MIN);
            zone.offset_from_utc_datetime(&midnight)
                .fix()
                .local_minus_utc()
                == 0
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    /// The days an events-side condition names, in order.
    fn named(skip: &str) -> Vec<String> {
        skip.split('\'')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn with_every_closed_day_rolled_the_events_side_reads_only_the_open_days() {
        // 90 days at noon: every closed day is rolled up, as bench-gen
        // leaves a data dir (#452).
        let now = day("2026-03-31")
            .and_hms_opt(12, 0, 0)
            .unwrap_or_default()
            .and_utc();
        let last = openlens_query::last_closed(now);
        let first = (now - Duration::days(90)).date_naive();
        let rolled: Vec<NaiveDate> = first.iter_days().take_while(|d| *d <= last).collect();
        let f = Filters::default();
        let (skip, w) = split(&rolled, now, 90, &f, "role");
        // The events: the day the window starts in, and the open days
        // (D-025) up to today; nothing else, and no day left out.
        let mut open = vec![first.format("%Y-%m-%d").to_string()];
        open.extend(
            last.succ_opt()
                .unwrap_or(last)
                .iter_days()
                .take_while(|d| *d <= now.date_naive())
                .map(|d| d.format("%Y-%m-%d").to_string()),
        );
        assert!(skip.starts_with(" AND date IN ("), "{skip}");
        assert_eq!(named(&skip), open);
        assert!(open.len() <= 4, "{open:?}");
        // The rollup: every other day, each read once.
        let w = w.unwrap_or_default();
        let from_rollup = named(w.split(')').next().unwrap_or_default());
        assert_eq!(from_rollup.len() + open.len(), 91);
        assert!(from_rollup.iter().all(|d| !open.contains(d)));
        // A day not rolled up yet (or rolled with another map) is read
        // from its events.
        let gap = rolled[40];
        let fewer: Vec<NaiveDate> = rolled.iter().copied().filter(|d| *d != gap).collect();
        let (skip, _) = split(&fewer, now, 90, &f, "role");
        assert!(named(&skip).contains(&gap.format("%Y-%m-%d").to_string()));
        // Nothing rolled: the events alone, with no day list.
        assert_eq!(split(&[], now, 90, &f, "role"), (String::new(), None));
    }

    #[test]
    fn only_zones_on_utc_midnight_share_the_rollup_days() {
        let (from, to) = (day("2026-01-05"), day("2026-01-20"));
        assert!(utc_days("UTC", from, to));
        // London is on UTC in winter, not across the spring change.
        assert!(utc_days("Europe/London", from, to));
        assert!(!utc_days(
            "Europe/London",
            day("2026-03-20"),
            day("2026-04-02")
        ));
        assert!(!utc_days("Europe/Paris", from, to));
        assert!(!utc_days("not a zone", from, to));
    }
}
