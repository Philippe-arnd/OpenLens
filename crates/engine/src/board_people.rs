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
    /// Closed days whose `person_day` rows were rolled with the map in
    /// force, oldest first.
    pub rolled: Vec<NaiveDate>,
    /// The board's clock: windows and days are cut at this instant.
    pub now: DateTime<Utc>,
}

impl People<'_> {
    /// A table of people rows for the filters `f` over the last `days`
    /// days: `distinct_id`, `environment`, `release`, `role` (canonical),
    /// `feature`, `events`, `last_ts` (the last event of the row) and
    /// `day` (the UTC day). A person active since a time inside the window
    /// is one with a row whose `last_ts` is at or after it.
    pub(crate) fn rows(&self, f: &Filters, days: i64) -> Result<String> {
        let (skip, rolled) = split(&self.rolled, self.now, days, f, &self.role);
        let events = format!(
            "SELECT distinct_id, environment, release, role, feature, count(*) AS events, \
             max(timestamp) AS last_ts, date AS day FROM {src} WHERE {w}{skip} \
             GROUP BY distinct_id, environment, release, role, feature, date",
            src = self.src,
            w = f.clause_at(days, self.now)?,
        );
        let Some(w) = rolled else {
            return Ok(format!("({events}) AS people"));
        };
        let role = &self.role;
        Ok(format!(
            "({events} UNION ALL SELECT person AS distinct_id, environment, release, \
             {role} AS role, feature, events, last_ts, date AS day FROM person_day WHERE {w}) AS people"
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
    let last = openlens_query::last_closed(now);
    let since = (now - Duration::days(90)).date_naive();
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
/// events off the rolled days entirely inside the window (empty when there
/// is none), and the rollup's WHERE clause for those days (none when no
/// day qualifies). `role` maps the rollup's raw role to the canonical one.
pub(crate) fn split(
    rolled: &[NaiveDate],
    now: DateTime<Utc>,
    days: i64,
    f: &Filters,
    role: &str,
) -> (String, Option<String>) {
    let cutoff = (now - Duration::days(days)).date_naive();
    let list = rolled
        .iter()
        .filter(|d| **d > cutoff)
        .map(|d| format!("'{}'", d.format("%Y-%m-%d")))
        .collect::<Vec<_>>()
        .join(", ");
    if list.is_empty() {
        return (String::new(), None);
    }
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
    (format!(" AND date NOT IN ({list})"), Some(w))
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
