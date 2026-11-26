//! A host allow-list for ingestion (#485, D-043): events whose page
//! address is on another host are dropped, so a copied snippet or a
//! scraper cannot write into a project.
//!
//! It never locks out ingestion through a typo:
//! - a list is saved after a preview of what it would have dropped over
//!   the last [`PREVIEW_HOURS`] hours ([`preview_allowed_hosts`]);
//! - it only drops anything while a host on it has sent events in the
//!   last [`ACTIVE_DAYS`] days; otherwise it is *pending* and every event
//!   passes, so a list naming no real host drops nothing, and a list left
//!   behind by a move to a new domain lets the new one in after a week;
//! - dropped events are counted per host ([`AllowState`]) and shown;
//! - an empty list is off, and events without a page address (server
//!   SDKs) are never dropped.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use openlens_event::{Event, host_of};
use openlens_query::QueryEngine;
use serde::Serialize;

use super::{Rules, check_pattern, covers, list::HOST_SQL};
use crate::{Result, int, text};

/// Hosts (or `*.suffix` patterns) an allow-list holds at most.
pub const MAX_ALLOWED: usize = 50;
/// A list drops events only while a host on it sent events this recently.
pub const ACTIVE_DAYS: i64 = 7;
/// The hours of events a preview counts.
pub const PREVIEW_HOURS: i64 = 24;
/// Hosts whose dropped events are counted one by one; the others are
/// counted together.
pub const MAX_DROPPED_HOSTS: usize = 100;
/// Hosts a preview reads at most (most events first).
const MAX_PREVIEW_HOSTS: usize = 2_000;

/// Replace the project's allow-list (engine command `set_allowed_hosts`).
/// Each entry is an exact host or `*.suffix`, as host rules take them;
/// blanks are skipped and duplicates kept once. An empty list turns it
/// off. Returns the list now stored; nothing changes on an error.
pub fn set_allowed_hosts(
    rules: &mut Rules,
    hosts: &[String],
) -> std::result::Result<Vec<String>, String> {
    let list = check_allowed(hosts)?;
    rules.allow.clone_from(&list);
    Ok(list)
}

/// An allow-list as stored, checked: see [`set_allowed_hosts`].
pub fn check_allowed(hosts: &[String]) -> std::result::Result<Vec<String>, String> {
    let mut list: Vec<String> = Vec::new();
    for raw in hosts {
        if raw.trim().is_empty() {
            continue;
        }
        let h = check_pattern(raw).map_err(|e| format!("{}: {e}", raw.trim()))?;
        if !list.contains(&h) {
            list.push(h);
        }
    }
    if list.len() > MAX_ALLOWED {
        return Err(format!("an allow-list holds at most {MAX_ALLOWED} hosts"));
    }
    Ok(list)
}

/// Whether `host` is on the list (exactly, or under a `*.suffix`).
#[must_use]
pub fn allows(allow: &[String], host: &str) -> bool {
    allow.iter().any(|p| covers(p, host))
}

/// Whether a list drops anything now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AllowStatus {
    /// No list: every event passes.
    Off,
    /// A list, but no host on it sent events in the last
    /// [`ACTIVE_DAYS`] days: every event passes.
    Pending,
    /// Events of hosts outside the list are dropped.
    Active,
}

/// The status of `allow` when a host on it was last seen at `last_listed`.
#[must_use]
pub fn allow_status(
    allow: &[String],
    last_listed: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> AllowStatus {
    if allow.is_empty() {
        AllowStatus::Off
    } else if last_listed.is_some_and(|t| now - t <= Duration::days(ACTIVE_DAYS)) {
        AllowStatus::Active
    } else {
        AllowStatus::Pending
    }
}

/// Events of one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostCount {
    /// The host.
    pub host: String,
    /// Events.
    pub events: u64,
}

/// What ingestion knows of a project's allow-list between batches: when a
/// host on it last sent events, and the events it dropped since `since`.
/// Kept in memory: a restart starts the counts again.
#[derive(Debug, Clone)]
pub struct AllowState {
    /// When a host on the list last sent events.
    pub last_listed: Option<DateTime<Utc>>,
    /// When the counts started.
    pub since: DateTime<Utc>,
    dropped: BTreeMap<String, u64>,
    other: u64,
}

impl AllowState {
    /// Counts starting at `now`, a listed host last seen at `last_listed`.
    #[must_use]
    pub fn new(now: DateTime<Utc>, last_listed: Option<DateTime<Utc>>) -> Self {
        Self {
            last_listed,
            since: now,
            dropped: BTreeMap::new(),
            other: 0,
        }
    }

    /// A host on the list was seen at `at` (the latest time wins).
    pub fn seen_listed(&mut self, at: Option<DateTime<Utc>>) {
        self.last_listed = self.last_listed.max(at);
    }

    /// Screen a batch (engine command run at ingestion): a listed host in
    /// it keeps the list active; then, if the list is active, events of a
    /// host outside it are dropped and counted. Events without a page
    /// address always pass. Returns the events dropped.
    pub fn screen(
        &mut self,
        allow: &[String],
        events: &mut Vec<Event>,
        now: DateTime<Utc>,
    ) -> usize {
        if allow.is_empty() {
            return 0;
        }
        let hosts: Vec<Option<String>> = events
            .iter()
            .map(|e| e.url.as_deref().and_then(host_of))
            .collect();
        if hosts.iter().flatten().any(|h| allows(allow, h)) {
            self.seen_listed(Some(now));
        }
        if allow_status(allow, self.last_listed, now) != AllowStatus::Active {
            return 0;
        }
        let mut keep = hosts
            .iter()
            .map(|h| h.as_deref().is_none_or(|h| allows(allow, h)));
        let before = events.len();
        events.retain(|_| keep.next().unwrap_or(true));
        for h in hosts.iter().flatten().filter(|h| !allows(allow, h)) {
            self.count(h);
        }
        before - events.len()
    }

    fn count(&mut self, host: &str) {
        if let Some(n) = self.dropped.get_mut(host) {
            *n += 1;
        } else if self.dropped.len() < MAX_DROPPED_HOSTS {
            self.dropped.insert(host.to_owned(), 1);
        } else {
            self.other += 1;
        }
    }

    /// Dropped events per host, most first, and those of hosts past
    /// [`MAX_DROPPED_HOSTS`] together.
    #[must_use]
    pub fn dropped(&self) -> (Vec<HostCount>, u64) {
        let mut v: Vec<HostCount> = self
            .dropped
            .iter()
            .map(|(host, events)| HostCount {
                host: host.clone(),
                events: *events,
            })
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.events));
        (v, self.other)
    }
}

/// A project's allow-list as the settings tab and the API show it.
#[derive(Debug, Clone, Serialize)]
pub struct AllowReport {
    /// The list; empty when off.
    pub hosts: Vec<String>,
    /// Whether it drops anything now.
    pub status: AllowStatus,
    /// When a host on it last sent events (RFC 3339), if known.
    pub last_listed: Option<String>,
    /// When the dropped counts started (RFC 3339): the last change of the
    /// list, or the server's start.
    pub dropped_since: String,
    /// Dropped events per host, most first.
    pub dropped: Vec<HostCount>,
    /// Dropped events of hosts not counted one by one.
    pub dropped_other: u64,
}

/// The report of `allow` with what ingestion counted (engine command
/// `allowed_hosts`).
#[must_use]
pub fn allow_report(
    allow: &[String],
    live: Option<&AllowState>,
    now: DateTime<Utc>,
) -> AllowReport {
    let last = live.and_then(|s| s.last_listed);
    let (dropped, dropped_other) = live.map(AllowState::dropped).unwrap_or_default();
    AllowReport {
        hosts: allow.to_vec(),
        status: allow_status(allow, last, now),
        last_listed: last.map(|t| t.to_rfc3339()),
        dropped_since: live.map_or(now, |s| s.since).to_rfc3339(),
        dropped,
        dropped_other,
    }
}

/// What a list would have done over the last [`PREVIEW_HOURS`] hours.
#[derive(Debug, Clone, Serialize)]
pub struct AllowPreview {
    /// The list, as it would be stored.
    pub hosts: Vec<String>,
    /// Whether it would drop anything once saved: `pending` when no host
    /// on it sent events in the last [`ACTIVE_DAYS`] days.
    pub status: AllowStatus,
    /// When a host on it last sent events (RFC 3339), within
    /// [`ACTIVE_DAYS`] days.
    pub last_listed: Option<String>,
    /// The same, for ingestion.
    #[serde(skip)]
    pub last_listed_at: Option<DateTime<Utc>>,
    /// Hours counted.
    pub hours: i64,
    /// Hosts outside the list and their events in those hours, most
    /// first: what an active list would have dropped.
    pub dropped: Vec<HostCount>,
    /// Their events in all.
    pub dropped_events: u64,
    /// Events those hours that it keeps: listed hosts, and events without
    /// a page address (never dropped).
    pub kept_events: u64,
}

/// The preview of `hosts` as an allow-list for `project` (engine command
/// `preview_allowed_hosts`): checked as [`set_allowed_hosts`] does, then
/// counted from the stored events of the last [`ACTIVE_DAYS`] days.
pub async fn preview_allowed_hosts(
    q: &QueryEngine,
    project: &str,
    hosts: &[String],
) -> Result<std::result::Result<AllowPreview, String>> {
    let list = match check_allowed(hosts) {
        Ok(l) => l,
        Err(e) => return Ok(Err(e)),
    };
    let since = (Utc::now() - Duration::days(ACTIVE_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let sql = format!(
        "SELECT COALESCE({HOST_SQL}, '') AS host, \
         SUM(CASE WHEN timestamp >= now() - INTERVAL '{PREVIEW_HOURS} hours' THEN 1 ELSE 0 END) AS recent, \
         date_part('epoch', MAX(timestamp)) AS last FROM events \
         WHERE date >= '{since}' AND timestamp >= now() - INTERVAL '{ACTIVE_DAYS} days' \
         GROUP BY 1 ORDER BY recent DESC LIMIT {MAX_PREVIEW_HOSTS}"
    );
    let r = q.sql(project, &sql).await?;
    let rows = r.rows.iter().map(|row| {
        let last = row
            .get("last")
            .and_then(serde_json::Value::as_f64)
            .and_then(|s| DateTime::from_timestamp_millis((s * 1000.0) as i64));
        (
            text(row, "host").unwrap_or_default(),
            u64::try_from(int(row, "recent")).unwrap_or(0),
            last,
        )
    });
    Ok(Ok(preview_from(list, rows)))
}

/// The preview of `list` from (host, events in the preview hours, last
/// event) rows; an empty host has no page address.
fn preview_from(
    list: Vec<String>,
    rows: impl IntoIterator<Item = (String, u64, Option<DateTime<Utc>>)>,
) -> AllowPreview {
    let mut out = AllowPreview {
        status: AllowStatus::Off,
        last_listed: None,
        last_listed_at: None,
        hours: PREVIEW_HOURS,
        dropped: Vec::new(),
        dropped_events: 0,
        kept_events: 0,
        hosts: list,
    };
    for (host, recent, last) in rows {
        if host.is_empty() || out.hosts.is_empty() {
            out.kept_events += recent;
        } else if allows(&out.hosts, &host) {
            out.kept_events += recent;
            out.last_listed_at = out.last_listed_at.max(last);
        } else if recent > 0 {
            out.dropped_events += recent;
            if out.dropped.len() < MAX_DROPPED_HOSTS {
                out.dropped.push(HostCount {
                    host,
                    events: recent,
                });
            }
        }
    }
    out.dropped.sort_by_key(|c| std::cmp::Reverse(c.events));
    // The query only reads the last ACTIVE_DAYS days: a listed host in it
    // makes the list active.
    out.status = if out.hosts.is_empty() {
        AllowStatus::Off
    } else if out.last_listed_at.is_some() {
        AllowStatus::Active
    } else {
        AllowStatus::Pending
    };
    out.last_listed = out.last_listed_at.map(|t| t.to_rfc3339());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(url: Option<&str>) -> Event {
        let mut e: Event = serde_json::from_value(serde_json::json!({
            "uuid": "x", "project": "p", "event": "$pageview", "distinct_id": "u", "properties": "{}",
            "timestamp": "2026-11-26T10:00:00Z", "received_at": "2026-11-26T10:00:00Z",
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        e.url = url.map(str::to_owned);
        e
    }

    fn list(hosts: &[&str]) -> Vec<String> {
        check_allowed(&hosts.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_pending_list_drops_nothing() {
        // A typo: no event ever comes from the host on the list.
        let allow = list(&["app.exmaple.com"]);
        let now = Utc::now();
        let mut s = AllowState::new(now, None);
        let mut batch = vec![
            event(Some("https://app.example.com/")),
            event(Some("https://other.example.org/")),
        ];
        assert_eq!(s.screen(&allow, &mut batch, now), 0);
        assert_eq!(batch.len(), 2);
        assert_eq!(
            allow_status(&allow, s.last_listed, now),
            AllowStatus::Pending
        );
        // A listed host seen more than a week ago no longer counts.
        let s = AllowState::new(now, Some(now - Duration::days(ACTIVE_DAYS + 1)));
        assert_eq!(
            allow_status(&allow, s.last_listed, now),
            AllowStatus::Pending
        );
        assert_eq!(allow_status(&[], Some(now), now), AllowStatus::Off);
    }

    #[test]
    fn an_active_list_drops_other_hosts_counts_them_and_lets_hostless_events_pass() {
        let allow = list(&["app.example.com", "*.preview.example.com"]);
        let now = Utc::now();
        let mut s = AllowState::new(now, None);
        let mut batch = vec![
            event(Some("https://app.example.com/a")),
            event(Some("https://pr-1.preview.example.com/")),
            event(Some("https://copycat.example.org/")),
            event(Some("https://copycat.example.org/b")),
            event(Some("https://evil.example.net:8080/")),
            event(None),
        ];
        // The listed host in the batch activates the list at once.
        assert_eq!(s.screen(&allow, &mut batch, now), 3);
        let kept: Vec<Option<&str>> = batch.iter().map(|e| e.url.as_deref()).collect();
        assert_eq!(
            kept,
            [
                Some("https://app.example.com/a"),
                Some("https://pr-1.preview.example.com/"),
                None
            ]
        );
        // A later batch with no listed host is still screened.
        let mut later = vec![event(Some("https://copycat.example.org/"))];
        assert_eq!(s.screen(&allow, &mut later, now), 1);
        let (dropped, other) = s.dropped();
        assert_eq!(
            dropped[0],
            HostCount {
                host: "copycat.example.org".into(),
                events: 3
            }
        );
        assert!(dropped.iter().any(|c| c.host == "evil.example.net"));
        assert_eq!(other, 0);
        let r = allow_report(&allow, Some(&s), now);
        assert_eq!(r.status, AllowStatus::Active);
        assert_eq!(r.dropped.len(), 2);
    }

    #[test]
    fn dropped_hosts_are_counted_within_a_bound() {
        let allow = list(&["app.example.com"]);
        let now = Utc::now();
        let mut s = AllowState::new(now, Some(now));
        let mut batch: Vec<Event> = (0..MAX_DROPPED_HOSTS + 5)
            .map(|i| event(Some(&format!("https://h{i}.example.org/"))))
            .collect();
        s.screen(&allow, &mut batch, now);
        let (dropped, other) = s.dropped();
        assert_eq!(dropped.len(), MAX_DROPPED_HOSTS);
        assert_eq!(other, 5);
    }

    #[test]
    fn the_preview_counts_what_would_be_dropped_and_kept() {
        let now = Utc::now();
        let rows = || {
            vec![
                ("app.example.com".to_owned(), 10, Some(now)),
                ("copycat.example.org".to_owned(), 4, Some(now)),
                (
                    "old.example.org".to_owned(),
                    0,
                    Some(now - Duration::days(3)),
                ),
                ("scraper.example.net".to_owned(), 7, Some(now)),
                (String::new(), 5, Some(now)),
            ]
        };
        let p = preview_from(list(&["App.Example.com"]), rows());
        assert_eq!(p.status, AllowStatus::Active);
        assert_eq!(p.kept_events, 15, "listed and hostless");
        assert_eq!(p.dropped_events, 11);
        assert_eq!(p.dropped[0].host, "scraper.example.net");
        assert!(!p.dropped.iter().any(|c| c.host == "old.example.org"));
        // A typo: everything would be outside, but the list stays pending.
        let p = preview_from(list(&["app.exmaple.com"]), rows());
        assert_eq!(p.status, AllowStatus::Pending);
        assert!(p.dropped_events > 0);
        // Off: nothing dropped.
        let p = preview_from(Vec::new(), rows());
        assert_eq!(p.status, AllowStatus::Off);
        assert_eq!(p.dropped_events, 0);
    }

    #[test]
    fn a_list_is_checked_and_bounded() {
        let mut r = Rules::default();
        let set = |r: &mut Rules, v: &[&str]| {
            set_allowed_hosts(r, &v.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>())
        };
        assert_eq!(
            set(&mut r, &[" App.example.com ", "", "app.example.com"]),
            Ok(vec!["app.example.com".to_owned()])
        );
        assert!(set(&mut r, &["bad host"]).is_err());
        assert_eq!(r.allow, ["app.example.com"], "unchanged on an error");
        let many: Vec<String> = (0..=MAX_ALLOWED)
            .map(|i| format!("h{i}.example.com"))
            .collect();
        assert!(set_allowed_hosts(&mut r, &many).is_err());
        assert_eq!(set(&mut r, &[]), Ok(Vec::new()));
        assert!(r.allow.is_empty());
    }
}
