//! Status announcements as commands (#562): what the team says about the
//! app's state, and the dated updates that follow a detected incident
//! (*investigating → identified → monitoring → resolved*).
//!
//! The text is plain: a body is kept as typed (at most [`MAX_BODY`]
//! characters, no control characters but line breaks and tabs) and every
//! page escapes it. At most one active *information* announcement is
//! pinned per project: pinning one unpins the others. Publishing an
//! announcement, an update or its end is also sent to the project's alert
//! channels when the project turned that on (`announcements.send`, off by
//! default, D-075).
//!
//! [`public`] is the read the public status page uses: the announcements
//! of the last [`PUBLIC_DAYS`] days, newest first, updates oldest first.

use openlens_meta::announcements::{Announcement, AnnouncementFields, IncidentRef};
use openlens_meta::{Meta, MetaError, Project};
use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};

/// Levels, from the calmest.
pub const LEVELS: [&str; 4] = ["information", "degraded", "outage", "resolved"];

/// States of an update, in the order an incident goes through them.
pub const STATES: [&str; 4] = ["investigating", "identified", "monitoring", "resolved"];

/// Longest title, in characters.
pub const MAX_TITLE: usize = 200;

/// Longest body (of an announcement or an update), in characters.
pub const MAX_BODY: usize = 4_000;

/// Most components one announcement names.
pub const MAX_COMPONENTS: usize = 20;

/// Days the public page shows.
pub const PUBLIC_DAYS: i64 = 90;

/// Days back the incidents are offered an announcement (as on Uptime).
const OFFER_DAYS: i64 = 30;

/// How far a start or end may be from now, in days.
const MAX_SHIFT_DAYS: i64 = 366;

/// The error text of an id that is not one of the project's
/// announcements (the API answers it with a 404).
pub const NO_SUCH: &str = "no such announcement";

/// The component an announcement concerns when none is named.
pub const PRODUCTION: &str = "production";

fn storage(e: MetaError) -> Error {
    match e {
        MetaError::Invalid(m) => Error::Invalid(m.to_owned()),
        e => Error::Storage(e.to_string()),
    }
}

/// No control characters but line breaks and tabs (in a body) or none
/// at all (in a title).
fn clean(s: &str, lines: bool) -> bool {
    s.chars()
        .all(|c| !c.is_control() || (lines && (c == '\n' || c == '\t')))
}

/// A title as stored: trimmed, one line, 1 to [`MAX_TITLE`] characters.
///
/// # Errors
/// What is wrong, for the person.
pub fn check_title(raw: &str) -> Result<String, Error> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > MAX_TITLE || !clean(t, false) {
        return Err(Error::Invalid(format!(
            "the title is one line of 1 to {MAX_TITLE} characters"
        )));
    }
    Ok(t.to_owned())
}

/// A body as stored: line breaks made `\n`, trimmed, at most
/// [`MAX_BODY`] characters, no control characters but line breaks and
/// tabs. Plain text: markup is kept as typed and shown as text.
///
/// # Errors
/// What is wrong, for the person.
pub fn check_body(raw: &str) -> Result<String, Error> {
    let b = raw.replace("\r\n", "\n").replace('\r', "\n");
    let b = b.trim();
    if b.chars().count() > MAX_BODY {
        return Err(Error::Invalid(format!(
            "the text is {MAX_BODY} characters at most"
        )));
    }
    if !clean(b, true) {
        return Err(Error::Invalid(
            "the text has control characters: plain text only".into(),
        ));
    }
    Ok(b.to_owned())
}

/// A level as stored.
///
/// # Errors
/// When it is not one of [`LEVELS`].
pub fn check_level(raw: &str) -> Result<String, Error> {
    let l = raw.trim().to_ascii_lowercase();
    if LEVELS.contains(&l.as_str()) {
        Ok(l)
    } else {
        Err(Error::Invalid(
            "the level is information, degraded, outage or resolved".into(),
        ))
    }
}

/// An update's state as stored.
///
/// # Errors
/// When it is not one of [`STATES`].
pub fn check_state(raw: &str) -> Result<String, Error> {
    let s = raw.trim().to_ascii_lowercase();
    if STATES.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(Error::Invalid(
            "the state is investigating, identified, monitoring or resolved".into(),
        ))
    }
}

/// Components as stored: environment names (`production` when none),
/// lowercased, without repeats, at most [`MAX_COMPONENTS`].
///
/// # Errors
/// A name that is not an environment name, or too many.
pub fn check_components(raw: &[String]) -> Result<Vec<String>, Error> {
    let mut out: Vec<String> = Vec::new();
    for c in raw.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
        let e = crate::environments::check_target(c)
            .map_err(|m| Error::Invalid(format!("component {c:?}: {m}")))?;
        if e == crate::environments::IGNORE {
            return Err(Error::Invalid(
                "a component is production or an environment name".into(),
            ));
        }
        if !out.contains(&e) {
            out.push(e);
        }
        if out.len() > MAX_COMPONENTS {
            return Err(Error::Invalid(format!(
                "{MAX_COMPONENTS} components at most"
            )));
        }
    }
    if out.is_empty() {
        out.push(PRODUCTION.to_owned());
    }
    Ok(out)
}

/// A time given by a caller, in Unix seconds: blank is `None`; Unix
/// seconds, RFC 3339, or a date and time (`2026-11-27T14:02`,
/// `2026-11-27 14:02[:05]`) in the time zone `tz`.
///
/// # Errors
/// When it is none of these, or more than a year from `now`.
pub fn parse_time(raw: &str, tz: &str, now: i64) -> Result<Option<i64>, Error> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let bad = || {
        Error::Invalid(format!(
            "{s:?} is not a time: RFC 3339, Unix seconds, or YYYY-MM-DD HH:MM in the project's time zone"
        ))
    };
    if s.len() > 40 {
        return Err(bad());
    }
    let t = if s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse::<i64>().map_err(|_| bad())?
    } else if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        t.timestamp()
    } else {
        let zone: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
        let naive = [
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%d %H:%M:%S",
        ]
        .iter()
        .find_map(|f| chrono::NaiveDateTime::parse_from_str(s, f).ok())
        .ok_or_else(bad)?;
        naive
            .and_local_timezone(zone)
            .earliest()
            .ok_or_else(bad)?
            .timestamp()
    };
    if (t - now).abs() > MAX_SHIFT_DAYS * 86_400 {
        return Err(Error::Invalid(
            "a start or end is within a year of now".into(),
        ));
    }
    Ok(Some(t))
}

/// Whether the announcement shows as current at `now`: started, and not
/// ended.
#[must_use]
pub fn active(a: &Announcement, now: i64) -> bool {
    a.fields.starts_at <= now && a.fields.ends_at.is_none_or(|e| e > now)
}

/// The pinned notice at `now`: the active, pinned information
/// announcement, if any.
#[must_use]
pub fn pinned(list: &[Announcement], now: i64) -> Option<&Announcement> {
    list.iter()
        .find(|a| a.fields.pinned && a.fields.level == "information" && active(a, now))
}

/// The announcements the public status page shows at `now`: those of the
/// last [`PUBLIC_DAYS`] days (started since, or not ended by then, or
/// scheduled), newest start first, each with its updates oldest first.
///
/// # Errors
/// When the store cannot be read.
pub fn public(meta: &Meta, project: &str, now: i64) -> Result<Vec<Announcement>, Error> {
    meta.announcements(project, now - PUBLIC_DAYS * 86_400)
        .map_err(storage)
}

/// Whether the project sends its announcements to its alert channels.
#[must_use]
pub fn sends(meta: &Meta, project: &str) -> bool {
    meta.project_setting(project, "announcements")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Sending>(&j).ok())
        .is_some_and(|s| s.send)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Sending {
    send: bool,
}

async fn project<H: Host>(host: &H, id: &str) -> Result<Project, Error> {
    host.project(id).await.ok_or(Error::NotFound)
}

fn tz_of(caller: &Caller, p: &Project) -> String {
    caller.tz.clone().unwrap_or_else(|| p.timezone.clone())
}

fn existing(meta: &Meta, project: &str, id: i64) -> Result<Announcement, Error> {
    meta.announcement(project, id)
        .map_err(storage)?
        .ok_or_else(|| Error::Invalid(NO_SUCH.into()))
}

/// The rules every stored announcement keeps, whoever set it.
fn check_fields(f: &AnnouncementFields) -> Result<(), Error> {
    if f.ends_at.is_some_and(|e| e <= f.starts_at) {
        return Err(Error::Invalid("the end is after the start".into()));
    }
    if f.pinned && f.level != "information" {
        return Err(Error::Invalid(
            "only an information announcement can be pinned".into(),
        ));
    }
    Ok(())
}

fn check_incident(meta: &Meta, project: &str, id: Option<i64>) -> Result<(), Error> {
    if let Some(i) = id
        && meta.incident_ref(project, i).map_err(storage)?.is_none()
    {
        return Err(Error::Invalid(format!("no incident {i} in this project")));
    }
    Ok(())
}

/// Send it to the alert channels, when the project turned that on.
async fn announce<H: Host>(host: &H, p: &Project, a: &Announcement, what: &str) {
    if !sends(host.meta(), &p.id) {
        return;
    }
    let title = format!("📣 {}: {}", p.name, a.fields.title);
    let mut body = format!("{what} ({})", a.fields.level);
    if let Some(u) = a.updates.last().filter(|_| what != "Announcement") {
        body.push_str(&format!("\n{}: {}", u.state, u.body));
    } else if !a.fields.body.is_empty() {
        body.push('\n');
        body.push_str(&a.fields.body);
    }
    body.push_str("\nSee Uptime in OpenLens.");
    let resolved = a.fields.level == "resolved";
    super::alerts::notify(host, &p.id, &title, &body, "announcement", resolved).await;
}

/// An announcement as listed: what is stored, and whether it shows now.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// The announcement.
    #[serde(flatten)]
    pub announcement: Announcement,
    /// Started and not ended.
    pub active: bool,
}

/// Something the Uptime page proposes about an incident.
#[derive(Debug, Clone, Serialize)]
pub struct Offer {
    /// `write_update`: an open incident no announcement follows;
    /// `mark_resolved`: an announcement whose incident closed while it
    /// is not resolved (it is never closed by itself).
    pub kind: &'static str,
    /// The incident.
    pub incident: IncidentRef,
    /// The announcement that follows it, for `mark_resolved`.
    pub announcement: Option<i64>,
}

/// What `announcements.list` returns.
#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    /// Newest start first, updates oldest first.
    pub announcements: Vec<Item>,
    /// The pinned notice now, if any.
    pub pinned: Option<i64>,
    /// What the page proposes about recent incidents.
    pub offers: Vec<Offer>,
    /// Announcements are also sent to the alert channels.
    pub send: bool,
}

/// The input of `announcements.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// Days back (1 to 400); [`PUBLIC_DAYS`] when absent.
    #[serde(default)]
    pub days: Option<i64>,
}

/// The announcements of a project, newest first, the pinned notice, and
/// what to propose about recent incidents.
///
/// # Errors
/// A `days` out of range, or a store that cannot be read.
pub fn listing(meta: &Meta, project: &str, days: i64, now: i64) -> Result<Listing, Error> {
    if !(1..=400).contains(&days) {
        return Err(Error::Invalid("days: 1 to 400".into()));
    }
    let list = meta
        .announcements(project, now - days * 86_400)
        .map_err(storage)?;
    let incidents = meta
        .incident_refs(project, now - OFFER_DAYS * 86_400)
        .map_err(storage)?;
    let mut offers = Vec::new();
    for i in incidents {
        let following: Vec<&Announcement> = list
            .iter()
            .filter(|a| a.fields.incident == Some(i.id))
            .collect();
        if following.is_empty() {
            if i.ended_at.is_none() {
                offers.push(Offer {
                    kind: "write_update",
                    incident: i,
                    announcement: None,
                });
            }
        } else if i.ended_at.is_some()
            && let Some(a) = following.iter().find(|a| a.fields.level != "resolved")
        {
            offers.push(Offer {
                kind: "mark_resolved",
                announcement: Some(a.id),
                incident: i,
            });
        }
    }
    Ok(Listing {
        pinned: pinned(&list, now).map(|a| a.id),
        announcements: list
            .into_iter()
            .map(|a| Item {
                active: active(&a, now),
                announcement: a,
            })
            .collect(),
        offers,
        send: sends(meta, project),
    })
}

/// `announcements.list`: the announcements of the last `days` (90 by
/// default), the pinned notice, the offers on incidents, and whether they
/// are sent to the alert channels.
pub struct List;

impl Command for List {
    const NAME: &'static str = "announcements.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Listing;

    async fn run<H: Host>(host: &H, _: &Caller, i: ListInput) -> Result<Listing, Error> {
        let p = project(host, &i.project).await?;
        let now = chrono::Utc::now().timestamp();
        listing(host.meta(), &p.id, i.days.unwrap_or(PUBLIC_DAYS), now)
    }
}

/// The input of `announcements.create`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// Title, one line.
    pub title: String,
    /// Body, plain text (may be empty).
    #[serde(default)]
    pub body: String,
    /// `information` (default), `degraded`, `outage` or `resolved`.
    #[serde(default)]
    pub level: Option<String>,
    /// `production` (default) or environment names.
    #[serde(default)]
    pub components: Vec<String>,
    /// Start: RFC 3339, Unix seconds or `YYYY-MM-DD HH:MM` in the
    /// project's zone; now when absent.
    #[serde(default)]
    pub start: Option<String>,
    /// End, the same way; none when absent.
    #[serde(default)]
    pub end: Option<String>,
    /// Pin it at the top of the page (information only; unpins the
    /// others).
    #[serde(default)]
    pub pinned: bool,
    /// The incident it follows (its id from `announcements.list`).
    #[serde(default)]
    pub incident: Option<i64>,
    /// When given, the first update's state (its text is the body, or
    /// the title when the body is empty).
    #[serde(default)]
    pub state: Option<String>,
}

/// Check a new announcement: its fields and its first update.
///
/// # Errors
/// What is wrong, for the person.
pub fn new_fields(
    i: &CreateInput,
    tz: &str,
    now: i64,
) -> Result<(AnnouncementFields, Option<(String, String)>), Error> {
    let f = AnnouncementFields {
        title: check_title(&i.title)?,
        body: check_body(&i.body)?,
        level: check_level(i.level.as_deref().unwrap_or("information"))?,
        components: check_components(&i.components)?,
        starts_at: parse_time(i.start.as_deref().unwrap_or(""), tz, now)?.unwrap_or(now),
        ends_at: parse_time(i.end.as_deref().unwrap_or(""), tz, now)?,
        pinned: i.pinned,
        incident: i.incident,
    };
    check_fields(&f)?;
    let first = match i.state.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => {
            let text = if f.body.is_empty() {
                f.title.clone()
            } else {
                f.body.clone()
            };
            Some((check_state(s)?, text))
        }
        None => None,
    };
    Ok((f, first))
}

/// `announcements.create`: post an announcement.
pub struct Create;

impl Command for Create {
    const NAME: &'static str = "announcements.create";
    const ACCESS: Access = Access::Write;
    type Input = CreateInput;
    type Output = Announcement;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        i: CreateInput,
    ) -> Result<Announcement, Error> {
        let p = project(host, &i.project).await?;
        let now = chrono::Utc::now().timestamp();
        let (f, first) = new_fields(&i, &tz_of(caller, &p), now)?;
        check_incident(host.meta(), &p.id, f.incident)?;
        let a = host
            .meta()
            .create_announcement(
                &p.id,
                &f,
                first.as_ref().map(|(s, b)| (s.as_str(), b.as_str())),
                now,
            )
            .map_err(storage)?;
        let what = if a.updates.is_empty() {
            "Announcement"
        } else {
            "New incident update"
        };
        announce(host, &p, &a, what).await;
        Ok(a)
    }
}

/// The input of `announcements.update`: only the fields given change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// The announcement.
    pub id: i64,
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// New body.
    #[serde(default)]
    pub body: Option<String>,
    /// New level.
    #[serde(default)]
    pub level: Option<String>,
    /// New components.
    #[serde(default)]
    pub components: Option<Vec<String>>,
    /// New start.
    #[serde(default)]
    pub start: Option<String>,
    /// New end; an empty text removes it.
    #[serde(default)]
    pub end: Option<String>,
    /// Pin or unpin.
    #[serde(default)]
    pub pinned: Option<bool>,
    /// The incident it follows.
    #[serde(default)]
    pub incident: Option<i64>,
}

/// The fields of `a` with the changes of `i` applied and checked. An
/// announcement that stops being information loses its pin.
///
/// # Errors
/// What is wrong, for the person.
pub fn changed_fields(
    a: &AnnouncementFields,
    i: &UpdateInput,
    tz: &str,
    now: i64,
) -> Result<AnnouncementFields, Error> {
    let mut f = a.clone();
    if let Some(t) = &i.title {
        f.title = check_title(t)?;
    }
    if let Some(b) = &i.body {
        f.body = check_body(b)?;
    }
    if let Some(l) = &i.level {
        f.level = check_level(l)?;
    }
    if let Some(c) = &i.components {
        f.components = check_components(c)?;
    }
    if let Some(s) = &i.start {
        f.starts_at = parse_time(s, tz, now)?.unwrap_or(f.starts_at);
    }
    if let Some(e) = &i.end {
        f.ends_at = parse_time(e, tz, now)?;
    }
    if i.incident.is_some() {
        f.incident = i.incident;
    }
    match i.pinned {
        Some(p) => f.pinned = p,
        None => f.pinned = f.pinned && f.level == "information",
    }
    check_fields(&f)?;
    Ok(f)
}

/// `announcements.update`: change an announcement (title, body, level,
/// components, times, pin). Not sent to the alert channels: an update of
/// the situation is `announcements.add_update`.
pub struct Update;

impl Command for Update {
    const NAME: &'static str = "announcements.update";
    const ACCESS: Access = Access::Write;
    type Input = UpdateInput;
    type Output = Announcement;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        i: UpdateInput,
    ) -> Result<Announcement, Error> {
        let p = project(host, &i.project).await?;
        let now = chrono::Utc::now().timestamp();
        let meta = host.meta();
        let a = existing(meta, &p.id, i.id)?;
        let f = changed_fields(&a.fields, &i, &tz_of(caller, &p), now)?;
        if f.incident != a.fields.incident {
            check_incident(meta, &p.id, f.incident)?;
        }
        meta.save_announcement(&p.id, a.id, &f, now)
            .map_err(storage)?
            .ok_or_else(|| Error::Invalid(NO_SUCH.into()))
    }
}

/// The input of `announcements.add_update`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddUpdateInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// The announcement.
    pub id: i64,
    /// `investigating`, `identified`, `monitoring` or `resolved`.
    pub state: String,
    /// What the team says, plain text (1 to 4,000 characters).
    pub body: String,
}

/// The fields of an announcement once `state` is posted: *resolved* makes
/// it resolved and ends it now (unless it ended before), unpinned.
#[must_use]
pub fn after_update(a: &AnnouncementFields, state: &str, now: i64) -> AnnouncementFields {
    let mut f = a.clone();
    if state == "resolved" {
        f.level = "resolved".into();
        f.pinned = false;
        if f.ends_at.is_none_or(|e| e > now) {
            f.ends_at = Some(now.max(f.starts_at + 1));
        }
    }
    f
}

/// `announcements.add_update`: post a dated update. A *resolved* update
/// also resolves and ends the announcement.
pub struct AddUpdate;

impl Command for AddUpdate {
    const NAME: &'static str = "announcements.add_update";
    const ACCESS: Access = Access::Write;
    type Input = AddUpdateInput;
    type Output = Announcement;

    async fn run<H: Host>(host: &H, _: &Caller, i: AddUpdateInput) -> Result<Announcement, Error> {
        let p = project(host, &i.project).await?;
        let now = chrono::Utc::now().timestamp();
        let meta = host.meta();
        let state = check_state(&i.state)?;
        let body = check_body(&i.body)?;
        if body.is_empty() {
            return Err(Error::Invalid("an update says something".into()));
        }
        let a = existing(meta, &p.id, i.id)?;
        let f = after_update(&a.fields, &state, now);
        let a = meta
            .add_announcement_update(&p.id, a.id, &state, &body, Some(&f), now)
            .map_err(storage)?
            .ok_or_else(|| Error::Invalid(NO_SUCH.into()))?;
        announce(host, &p, &a, "Incident update").await;
        Ok(a)
    }
}

/// The input of `announcements.close`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloseInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// The announcement.
    pub id: i64,
    /// A closing word, posted as a *resolved* update.
    #[serde(default)]
    pub body: Option<String>,
}

/// `announcements.close`: end an announcement now (*Mark resolved*): a
/// degraded or outage one becomes resolved, the pin goes, and one that
/// follows updates gets a *resolved* update (the text given, or
/// "Resolved.").
pub struct Close;

impl Command for Close {
    const NAME: &'static str = "announcements.close";
    const ACCESS: Access = Access::Write;
    type Input = CloseInput;
    type Output = Announcement;

    async fn run<H: Host>(host: &H, _: &Caller, i: CloseInput) -> Result<Announcement, Error> {
        let p = project(host, &i.project).await?;
        let now = chrono::Utc::now().timestamp();
        let meta = host.meta();
        let a = existing(meta, &p.id, i.id)?;
        let body = check_body(i.body.as_deref().unwrap_or(""))?;
        let mut f = after_update(&a.fields, "resolved", now);
        if a.fields.level == "information" {
            f.level = "information".into();
        }
        let last_resolved = a.updates.last().is_some_and(|u| u.state == "resolved");
        let a = if !body.is_empty() || (!a.updates.is_empty() && !last_resolved) {
            let text = if body.is_empty() { "Resolved." } else { &body };
            meta.add_announcement_update(&p.id, a.id, "resolved", text, Some(&f), now)
        } else {
            meta.save_announcement(&p.id, a.id, &f, now)
        }
        .map_err(storage)?
        .ok_or_else(|| Error::Invalid(NO_SUCH.into()))?;
        announce(host, &p, &a, "Closed").await;
        Ok(a)
    }
}

/// The input of `announcements.delete`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// The announcement.
    pub id: i64,
}

/// `announcements.delete`: remove an announcement and its updates.
pub struct Delete;

impl Command for Delete {
    const NAME: &'static str = "announcements.delete";
    const ACCESS: Access = Access::Write;
    type Input = DeleteInput;
    type Output = ();

    async fn run<H: Host>(host: &H, _: &Caller, i: DeleteInput) -> Result<(), Error> {
        let p = project(host, &i.project).await?;
        if host
            .meta()
            .delete_announcement(&p.id, i.id)
            .map_err(storage)?
        {
            Ok(())
        } else {
            Err(Error::Invalid(NO_SUCH.into()))
        }
    }
}

/// The input of `announcements.send`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SendInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// Also send announcements, their updates and their end to the alert
    /// channels.
    pub send: bool,
}

/// `announcements.send`: turn sending announcements to the project's
/// alert channels on or off (off by default, D-075).
pub struct SetSend;

impl Command for SetSend {
    const NAME: &'static str = "announcements.send";
    const ACCESS: Access = Access::Write;
    type Input = SendInput;
    type Output = bool;

    async fn run<H: Host>(host: &H, _: &Caller, i: SendInput) -> Result<bool, Error> {
        let p = project(host, &i.project).await?;
        let j = serde_json::to_string(&Sending { send: i.send })
            .map_err(|e| Error::Storage(e.to_string()))?;
        host.meta()
            .set_project_setting(&p.id, "announcements", &j)
            .map_err(storage)?;
        Ok(i.send)
    }
}
