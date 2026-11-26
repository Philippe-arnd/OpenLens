//! Planned maintenance as commands (#563, D-075): list a project's
//! windows, schedule one (optionally announced once on the alert
//! channels), extend it, end it now or cancel it before it starts. The
//! Uptime page, the API, the CLI and MCP run them; the adapters write the
//! audit entry.
//!
//! Times are given as `now`, RFC 3339 (`2026-12-03T22:00:00+01:00`) or a
//! local `YYYY-MM-DD HH:MM` in the caller's zone, else the project's
//! (D-017).

use chrono::TimeZone;
use openlens_meta::maintenance::NewWindow;
use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::maintenance::{PRODUCTION, Window, describe, length, state_at};

/// Planned or running windows a project may have at once.
pub const MAX_OPEN: i64 = 50;
/// Longest window: three days.
pub const MAX_SECONDS: i64 = 72 * 3600;
/// How far ahead a window may start.
pub const MAX_AHEAD_DAYS: i64 = 366;
/// Longest text, in characters.
pub const MAX_TEXT: usize = 500;
/// Most components one window covers.
pub const MAX_COMPONENTS: usize = 10;
/// A start this far in the past still reads as now (a clock or a form a
/// little late).
const LATE: i64 = 300;
/// Ended windows that `maintenance.list` shows.
const LIST_DAYS: i64 = 30;

/// A window as the commands answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaintenanceWindow {
    /// Id.
    pub id: i64,
    /// Start (Unix seconds).
    pub start: i64,
    /// End (Unix seconds, excluded).
    pub end: i64,
    /// `planned`, `running` or `ended`.
    pub state: &'static str,
    /// What it covers: `production` or environment names.
    pub components: Vec<String>,
    /// What is done.
    pub text: String,
    /// Its dates in the caller's zone (`3 Dec, 22:00–22:30`).
    pub when: String,
    /// Its length in words.
    pub length: String,
    /// Who scheduled it.
    pub created_by: String,
    /// Whether it was announced on the alert channels.
    pub announced: bool,
}

impl MaintenanceWindow {
    /// The window `w` at `now`, its dates in `tz`.
    #[must_use]
    pub fn of(w: &Window, now: i64, tz: &str) -> Self {
        Self {
            id: w.id,
            start: w.starts_at,
            end: w.ends_at,
            state: state_at(w, now),
            components: w.components.clone(),
            text: w.text.clone(),
            when: describe(w, tz),
            length: length(w.ends_at - w.starts_at),
            created_by: w.created_by.clone(),
            announced: w.announced_at.is_some(),
        }
    }
}

/// The zone times are read and shown in: the caller's, else the
/// project's.
fn zone(caller: &Caller, project_tz: &str) -> String {
    caller
        .tz
        .clone()
        .filter(|t| t.parse::<chrono_tz::Tz>().is_ok())
        .unwrap_or_else(|| project_tz.to_owned())
}

/// A time given by a person or an agent, as Unix seconds: `now`, RFC 3339,
/// or `YYYY-MM-DD HH:MM[:SS]` (or with a `T`) in the zone `tz`.
///
/// # Errors
/// What is wrong, for the person.
pub fn parse_time(raw: &str, tz: &str, now: i64) -> Result<i64, String> {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("now") {
        return Ok(now);
    }
    if t.len() <= 40 {
        if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
            return Ok(d.timestamp());
        }
        let zone: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
        let local = t.replace('T', " ");
        for f in ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"] {
            if let Ok(n) = chrono::NaiveDateTime::parse_from_str(&local, f) {
                return zone
                    .from_local_datetime(&n)
                    .earliest()
                    .map(|d| d.timestamp())
                    .ok_or_else(|| format!("{t} does not exist in {tz} (a clock change)"));
            }
        }
    }
    Err(format!(
        "a time is now, YYYY-MM-DD HH:MM (in {tz}) or RFC 3339; got {}",
        t.chars().take(40).collect::<String>()
    ))
}

/// The end of a window from `end` (a time) or `minutes` after `from`.
fn end_of(
    end: Option<&str>,
    minutes: Option<u32>,
    from: i64,
    tz: &str,
    now: i64,
) -> Result<i64, Error> {
    match (end.filter(|e| !e.trim().is_empty()), minutes) {
        (Some(e), None) => parse_time(e, tz, now).map_err(Error::Invalid),
        (None, Some(m)) if (1..=72 * 60).contains(&m) => Ok(from + i64::from(m) * 60),
        (None, Some(_)) => Err(Error::Invalid("minutes: 1 to 4320 (three days)".into())),
        (Some(_), Some(_)) => Err(Error::Invalid("give an end or minutes, not both".into())),
        (None, None) => Err(Error::Invalid("give an end or minutes".into())),
    }
}

/// Check a window's start and end against `now`.
fn check_span(start: i64, end: i64) -> Result<(), Error> {
    if end <= start {
        return Err(Error::Invalid("a window ends after it starts".into()));
    }
    if end - start > MAX_SECONDS {
        return Err(Error::Invalid("a window lasts three days at most".into()));
    }
    Ok(())
}

/// The components of a window, checked: `production` or environment
/// names, at most [`MAX_COMPONENTS`], none repeated. Empty means
/// production.
fn check_components(raw: &[String]) -> Result<Vec<String>, Error> {
    let mut out: Vec<String> = Vec::new();
    for c in raw {
        let name = crate::environments::check_target(c).map_err(|e| Error::Invalid(e.into()))?;
        if name == crate::environments::IGNORE {
            return Err(Error::Invalid("ignore is not a component".into()));
        }
        if !out.contains(&name) {
            out.push(name);
        }
        if out.len() > MAX_COMPONENTS {
            return Err(Error::Invalid(format!(
                "at most {MAX_COMPONENTS} components"
            )));
        }
    }
    if out.is_empty() {
        out.push(PRODUCTION.to_owned());
    }
    Ok(out)
}

fn check_text(raw: &str) -> Result<String, Error> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > MAX_TEXT || t.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid(format!(
            "the text is 1 to {MAX_TEXT} characters on one line"
        )));
    }
    Ok(t.to_owned())
}

/// The input of `maintenance.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
}

/// `maintenance.list`: the project's planned and running windows, and
/// those ended in the last 30 days, by start.
pub struct List;

impl Command for List {
    const NAME: &'static str = "maintenance.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Vec<MaintenanceWindow>;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: ListInput,
    ) -> Result<Vec<MaintenanceWindow>, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let now = chrono::Utc::now().timestamp();
        Ok(list(host, &p.id, &zone(caller, &p.timezone), now))
    }
}

/// The windows `maintenance.list` answers, at `now`, dates in `tz`.
#[must_use]
pub fn list<H: Host>(host: &H, project: &str, tz: &str, now: i64) -> Vec<MaintenanceWindow> {
    host.meta()
        .maintenance_windows(
            project,
            now - LIST_DAYS * 86400,
            now + (MAX_AHEAD_DAYS + 4) * 86400,
        )
        .unwrap_or_default()
        .iter()
        .map(|w| MaintenanceWindow::of(w, now, tz))
        .collect()
}

/// The input of `maintenance.schedule`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleInput {
    /// Project id.
    pub project: String,
    /// Start: `now` (the default), RFC 3339, or `YYYY-MM-DD HH:MM` in the
    /// caller's zone.
    pub start: Option<String>,
    /// End, as `start`; or give `minutes`.
    pub end: Option<String>,
    /// Length in minutes (1 to 4320), instead of `end`.
    pub minutes: Option<u32>,
    /// `production` (the default) and/or environment names.
    pub components: Vec<String>,
    /// What is done, shown with the window.
    pub text: String,
    /// Announce it once on the alert channels now.
    pub announce: bool,
    /// Who schedules it (set by the adapter, for the window's record).
    pub by: String,
}

/// `maintenance.schedule`: declare a window ahead (or starting now).
pub struct Schedule;

impl Command for Schedule {
    const NAME: &'static str = "maintenance.schedule";
    const ACCESS: Access = Access::Write;
    type Input = ScheduleInput;
    type Output = MaintenanceWindow;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: ScheduleInput,
    ) -> Result<MaintenanceWindow, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let tz = zone(caller, &p.timezone);
        let now = chrono::Utc::now().timestamp();
        let start = match input.start.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => parse_time(s, &tz, now).map_err(Error::Invalid)?,
            None => now,
        };
        if start < now - LATE {
            return Err(Error::Invalid(
                "the start is in the past (give now, or a later time)".into(),
            ));
        }
        // A start a few minutes late reads as now.
        let start = start.max(now);
        if start > now + MAX_AHEAD_DAYS * 86400 {
            return Err(Error::Invalid("a window starts within a year".into()));
        }
        let end = end_of(input.end.as_deref(), input.minutes, start, &tz, now)?;
        check_span(start, end)?;
        let components = check_components(&input.components)?;
        let text = check_text(&input.text)?;
        let meta = host.meta();
        if meta.open_maintenance_count(&p.id, now).unwrap_or(0) >= MAX_OPEN {
            return Err(Error::Invalid(format!(
                "at most {MAX_OPEN} planned or running windows; cancel one first"
            )));
        }
        let by: String = input.by.trim().chars().take(200).collect();
        let new = NewWindow {
            starts_at: start,
            ends_at: end,
            components: &components,
            text: &text,
            created_by: if by.is_empty() { "unknown" } else { &by },
            now,
        };
        let id = meta
            .add_maintenance(&p.id, &new)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut w = meta
            .maintenance_window(&p.id, id)
            .map_err(|e| Error::Storage(e.to_string()))?
            .ok_or_else(|| Error::Storage("the window was not stored".into()))?;
        if input.announce {
            // The channels' readers are the team: the project's zone, named.
            let title = format!(
                "🛠 {}: planned maintenance on {} ({})",
                p.name,
                describe(&w, &p.timezone),
                p.timezone
            );
            let body = format!(
                "{text}\nCovers: {}. Alerts for it are held during the window, and its time is not counted against availability.",
                components.join(", ")
            );
            super::alerts::notify(host, &p.id, &title, &body, "maintenance", false).await;
            let _ = meta.maintenance_announced(&p.id, id, now);
            w.announced_at = Some(now);
        }
        Ok(MaintenanceWindow::of(&w, now, &tz))
    }
}

/// The input of `maintenance.extend`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtendInput {
    /// Project id.
    pub project: String,
    /// The window's id.
    pub id: i64,
    /// The new end, as a time; or give `minutes`.
    pub end: Option<String>,
    /// Minutes added to its current end (1 to 4320).
    pub minutes: Option<u32>,
}

/// `maintenance.extend`: move the end of a planned or running window.
pub struct Extend;

impl Command for Extend {
    const NAME: &'static str = "maintenance.extend";
    const ACCESS: Access = Access::Write;
    type Input = ExtendInput;
    type Output = MaintenanceWindow;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: ExtendInput,
    ) -> Result<MaintenanceWindow, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let tz = zone(caller, &p.timezone);
        let now = chrono::Utc::now().timestamp();
        let w = window(host, &p.id, input.id)?;
        if w.ends_at <= now {
            return Err(Error::Invalid("this window has ended".into()));
        }
        let end = end_of(input.end.as_deref(), input.minutes, w.ends_at, &tz, now)?;
        if end <= now {
            return Err(Error::Invalid("the new end is in the past".into()));
        }
        check_span(w.starts_at, end)?;
        set_end(host, &p.id, &w, end, now, &tz)
    }
}

/// A window's id, in a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInput {
    /// Project id.
    pub project: String,
    /// The window's id.
    pub id: i64,
}

/// `maintenance.end_now`: end a running window now; its incidents that
/// are still failing are announced from now.
pub struct EndNow;

impl Command for EndNow {
    const NAME: &'static str = "maintenance.end_now";
    const ACCESS: Access = Access::Write;
    type Input = WindowInput;
    type Output = MaintenanceWindow;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: WindowInput,
    ) -> Result<MaintenanceWindow, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let now = chrono::Utc::now().timestamp();
        let w = window(host, &p.id, input.id)?;
        match state_at(&w, now) {
            // Ended in the second it started, it still lasts one.
            "running" => {
                let end = now.max(w.starts_at + 1);
                set_end(host, &p.id, &w, end, now, &zone(caller, &p.timezone))
            }
            "planned" => Err(Error::Invalid(
                "this window has not started; cancel it instead".into(),
            )),
            _ => Err(Error::Invalid("this window has ended".into())),
        }
    }
}

/// `maintenance.cancel`: remove a window that has not started.
pub struct Cancel;

impl Command for Cancel {
    const NAME: &'static str = "maintenance.cancel";
    const ACCESS: Access = Access::Write;
    type Input = WindowInput;
    type Output = MaintenanceWindow;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: WindowInput,
    ) -> Result<MaintenanceWindow, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let now = chrono::Utc::now().timestamp();
        let w = window(host, &p.id, input.id)?;
        if state_at(&w, now) != "planned" {
            return Err(Error::Invalid(
                "only a window that has not started can be cancelled; end it now instead".into(),
            ));
        }
        host.meta()
            .delete_maintenance(&p.id, w.id)
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(MaintenanceWindow::of(&w, now, &zone(caller, &p.timezone)))
    }
}

fn window<H: Host>(host: &H, project: &str, id: i64) -> Result<Window, Error> {
    host.meta()
        .maintenance_window(project, id)
        .map_err(|e| Error::Storage(e.to_string()))?
        .ok_or_else(|| Error::Invalid(format!("no maintenance window {id}")))
}

fn set_end<H: Host>(
    host: &H,
    project: &str,
    w: &Window,
    end: i64,
    now: i64,
    tz: &str,
) -> Result<MaintenanceWindow, Error> {
    let stored = host
        .meta()
        .set_maintenance_end(project, w.id, end)
        .map_err(|e| Error::Storage(e.to_string()))?;
    if !stored {
        return Err(Error::Invalid("a window ends after it starts".into()));
    }
    let w = Window {
        ends_at: end,
        ..w.clone()
    };
    Ok(MaintenanceWindow::of(&w, now, tz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_in_the_given_zone_and_refuse_nonsense() {
        let now = 1_000;
        assert_eq!(parse_time("now", "UTC", now), Ok(now));
        let utc = parse_time("2026-12-03 22:00", "UTC", now);
        let paris = parse_time("2026-12-03T22:00", "Europe/Paris", now);
        assert!(
            matches!((utc, paris), (Ok(u), Ok(p)) if p < u),
            "Paris is ahead of UTC"
        );
        assert_eq!(
            parse_time("2026-12-03T22:00:00+00:00", "Europe/Paris", now),
            parse_time("2026-12-03 22:00", "UTC", now)
        );
        assert!(parse_time("tomorrow", "UTC", now).is_err());
        assert!(parse_time(&"9".repeat(10_000), "UTC", now).is_err());
    }

    #[test]
    fn components_default_to_production_and_are_checked() {
        assert_eq!(check_components(&[]), Ok(vec![PRODUCTION.to_owned()]));
        let twice = ["Staging".to_owned(), "staging".to_owned()];
        assert_eq!(check_components(&twice).map(|c| c.len()), Ok(1));
        assert!(check_components(&["ignore".to_owned()]).is_err());
        assert!(check_components(&["a b".to_owned()]).is_err());
        let many: Vec<String> = (0..20).map(|i| format!("e{i}")).collect();
        assert!(check_components(&many).is_err());
    }
}
