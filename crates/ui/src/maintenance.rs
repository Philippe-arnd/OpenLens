//! The planned maintenance card of the Uptime page (#563, D-076): the
//! windows planned, running and recently ended, and for admins the form
//! that schedules one and the buttons that extend, end or cancel it.

use super::*;

/// One window as shown.
#[derive(Debug, Clone, Default)]
pub struct MaintenanceRow {
    /// Id.
    pub id: i64,
    /// Its dates in the reader's zone (`3 Dec, 22:00–22:30`).
    pub when: String,
    /// Its length in words.
    pub length: String,
    /// `planned`, `running` or `ended`.
    pub state: String,
    /// What it covers, comma-separated.
    pub components: String,
    /// What is done.
    pub text: String,
}

/// The card.
#[derive(Debug, Clone, Default)]
pub struct MaintenanceView {
    /// The windows, by start.
    pub rows: Vec<MaintenanceRow>,
    /// The reader may change them.
    pub admin: bool,
    /// The reader's zone, named next to the form's start.
    pub tz: String,
}

/// The planned maintenance card.
pub fn maintenance_card(project: &str, v: &MaintenanceView) -> Markup {
    let base = format!("/p/{project}/uptime/maintenance");
    html! {
        section.card.wide id="maintenance" {
            h2.sky { (icon("clock")) "Planned maintenance" }
            @if v.rows.is_empty() {
                p.muted { "None planned. During a window, its components show " em { "under maintenance" } ", their incidents are recorded but not announced, and the time is not counted against availability." }
            } @else {
                div.table-scroll { table.plain {
                    thead { tr { th { "When" } th { "Covers" } th { "What" } th { span.sr-only { "Actions" } } } }
                    tbody { @for r in &v.rows {
                        tr {
                            td.small {
                                span class={ "status " (match r.state.as_str() { "running" => "warn", "planned" => "good", _ => "muted" }) } {
                                    (match r.state.as_str() { "running" => "under maintenance", "planned" => "planned", _ => "ended" })
                                }
                                " " (r.when) " · " (r.length)
                            }
                            td.small { (r.components) }
                            td.small { (r.text) }
                            td { @if v.admin && r.state != "ended" {
                                form.inline method="post" action={ (base) "/" (r.id) "/extend" } {
                                    input type="hidden" name="minutes" value="30";
                                    button.ghost type="submit" { "+30 min" }
                                }
                                @if r.state == "running" {
                                    form.inline method="post" action={ (base) "/" (r.id) "/end" } { button.ghost type="submit" { "End now" } }
                                } @else {
                                    form.inline method="post" action={ (base) "/" (r.id) "/cancel" } { button.ghost type="submit" { "Cancel" } }
                                }
                            } }
                        }
                    } }
                } }
            }
            @if v.admin {
                form.settings method="post" action=(base) {
                    label { "Start (" (v.tz) ", empty for now) " input type="datetime-local" name="start"; }
                    label { "Minutes " input.narrow type="number" name="minutes" min="1" max="4320" value="30" required; }
                    label { "Covers " input type="text" name="components" value="production" maxlength="700" placeholder="production, staging"; }
                    label { "What is done " input type="text" name="text" maxlength="500" required placeholder="Database migration"; }
                    label { input type="checkbox" name="announce"; " Announce it once on the alert channels" }
                    button type="submit" { "Schedule" }
                }
                p.muted.small { "An agent or a CI step can open one right before a deploy and end it after: " code { "openlens-cli maintenance schedule --minutes 15 --text \"Deploy\"" } "." }
            }
        }
    }
}
