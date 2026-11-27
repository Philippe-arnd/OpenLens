//! The Jobs card of the Uptime page (#609, D-086): each cron or
//! background job that checks in, with its last check-in, its schedule
//! (declared or learned) and its state; when there is none yet, how to add
//! a check-in from a shell script or a server SDK. For admins, the switch
//! that counts jobs in availability and a button to remove a job.

use super::*;

/// One job as shown.
#[derive(Debug, Clone, Default)]
pub struct CheckInRow {
    /// Its slug.
    pub slug: String,
    /// `learning`, `ok`, `missed`, `failed` or `slower`.
    pub state: &'static str,
    /// The last check-in, in the reader's zone.
    pub last: Option<String>,
    /// The last check-in said it succeeded.
    pub last_ok: Option<bool>,
    /// The last run's duration, ms.
    pub last_ms: Option<i64>,
    /// The gap expected between runs, in words.
    pub every: Option<String>,
    /// The gap was declared by the job, not learned.
    pub declared: bool,
    /// Its usual duration, ms.
    pub usual: Option<i64>,
    /// Check-ins still needed before the schedule is learned.
    pub to_learn: usize,
}

/// The card.
#[derive(Debug, Clone, Default)]
pub struct CheckInsView {
    /// The jobs, by slug.
    pub rows: Vec<CheckInRow>,
    /// The project counts its jobs in availability.
    pub in_availability: bool,
    /// Jobs a project may have.
    pub max: i64,
    /// The reader may change the switch and remove jobs.
    pub admin: bool,
    /// This server's address, for the `curl` line.
    pub base: String,
}

fn state_tag(state: &str) -> Markup {
    let (tone, words) = match state {
        "missed" => ("bad", "Did not run"),
        "failed" => ("bad", "Failed"),
        "slower" => ("warn", "Slower"),
        "learning" => ("info", "Learning"),
        _ => ("good", "On time"),
    };
    html! { span class={ "tag tone-" (tone) } { (words) } }
}

/// The Jobs card.
pub fn check_ins_card(p: &ProjectInfo<'_>, v: &CheckInsView) -> Markup {
    let base = format!("/p/{}/uptime/jobs", p.id);
    let curl = format!(
        "curl -fsS -X POST {}/check-in/{}/nightly-export",
        v.base, p.key
    );
    html! {
        section.card.table-card.wide id="jobs" {
            h2 { (icon("task")) "Jobs" }
            @if v.rows.is_empty() {
                div.pad {
                    p.muted { "No job has checked in yet. A cron job, a queue worker or a nightly export that stops sends no request and raises no error, so nothing else here would notice. Make it check in at the end of each run: after four check-ins OpenLens knows how often it runs, and alerts when a run is missed (no check-in for one and a half times the usual gap), fails, or takes twice its usual time." }
                    p.small { "From a shell script or a crontab line, after the job:" }
                    pre.snippet { code { (curl) } }
                    p.small { "From a server SDK (Node shown; Python, Go, Ruby and PHP have the same call):" }
                    pre.snippet { code { "ol.checkIn(\"nightly-export\", { status: \"ok\", ms: 1234 });" } }
                    p.small.muted { "Add " code { "?status=error" } " when the run failed, " code { "&ms=" } " for its duration, and " code { "expectEvery" } " (seconds) to give its schedule instead of letting it learn." }
                }
            } @else {
                div.table-scroll { table {
                    thead { tr { th { "Job" } th { "State" } th { "Last check-in" } th { "Schedule" } th { "Usual time" } th { span.sr-only { "Actions" } } } }
                    tbody { @for r in &v.rows {
                        tr {
                            td { span.mono { (r.slug) } }
                            td { (state_tag(r.state)) }
                            td.small {
                                @if let Some(t) = &r.last { (t) } @else { "—" }
                                @if r.last_ok == Some(false) { " · error" }
                                @if let Some(m) = r.last_ms { " · " (m) " ms" }
                            }
                            td.small {
                                @if let Some(e) = &r.every {
                                    "every " (e) @if r.declared { " · declared" } @else { " · learned" }
                                } @else {
                                    "learning: " (r.to_learn) " more check-in" @if r.to_learn != 1 { "s" }
                                }
                            }
                            td.small { @if let Some(u) = r.usual { (u) " ms" } @else { "—" } }
                            td { @if v.admin {
                                form method="post" action={ (base) "/" (r.slug) "/remove" } {
                                    button.btn.sm.ghost type="submit" { "Remove" }
                                }
                            } }
                        }
                    } }
                } }
                p.muted.small.pad {
                    @if v.in_availability { "Counted in availability: each check-in is a check, each missed run a failed one. " }
                    @else { "Not counted in availability. " }
                    "At most " (v.max) " jobs; a removed job starts learning again at its next check-in."
                }
            }
            @if v.admin {
                form.inline.pad method="post" action={ (base) "/availability" } {
                    label.check {
                        input type="checkbox" name="on" checked[v.in_availability];
                        " Count jobs in the availability of the last 24 hours and 7 days"
                    }
                    button.btn.sm type="submit" { "Save" }
                }
            }
        }
    }
}
