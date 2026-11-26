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
        section.card.table-card.wide id="maintenance" {
            h2 { (icon("clock")) "Planned maintenance" }
            @if v.rows.is_empty() {
                p.muted.pad { "None planned. During a window, its components show " em { "under maintenance" } ", their incidents are recorded but not announced, and the time is not counted against availability." }
            } @else {
                div.table-scroll { table {
                    thead { tr { th { "State" } th { "When" } th { "Covers" } th { "What" } th { span.sr-only { "Actions" } } } }
                    tbody { @for r in &v.rows {
                        tr {
                            td { span class={ "tag tone-" (match r.state.as_str() { "running" => "warn", "planned" => "info", _ => "muted" }) } {
                                (match r.state.as_str() { "running" => "Under maintenance", "planned" => "Planned", _ => "Ended" })
                            } }
                            td.small { (r.when) " · " (r.length) }
                            td.small { (r.components) }
                            td.small { (r.text) }
                            td { @if v.admin && r.state != "ended" {
                                div.row-actions {
                                    form method="post" action={ (base) "/" (r.id) "/extend" } {
                                        input type="hidden" name="minutes" value="30";
                                        button.btn.sm type="submit" { "+30 min" }
                                    }
                                    @if r.state == "running" {
                                        form method="post" action={ (base) "/" (r.id) "/end" } { button.btn.sm type="submit" { "End now" } }
                                    } @else {
                                        form method="post" action={ (base) "/" (r.id) "/cancel" } { button.btn.sm.ghost type="submit" { "Cancel" } }
                                    }
                                }
                            } }
                        }
                    } }
                } }
            }
        }
        @if v.admin {
            form.card.form-card.wide method="post" action=(base) {
                h2 { "Plan a maintenance window" }
                div.form-grid {
                    label.field { "Start"
                        input type="datetime-local" name="start";
                        small { "In " (v.tz) ". Empty: now." }
                    }
                    label.field { "Length (minutes)"
                        input type="number" name="minutes" min="1" max="4320" value="30" required;
                        small { "At most 3 days. It can be extended or ended early." }
                    }
                    label.field { "Covers"
                        input type="text" name="components" value="production" maxlength="700" placeholder="production, staging";
                        small { "production or environment names, comma-separated." }
                    }
                    label.field { "What is done"
                        input type="text" name="text" maxlength="500" required placeholder="Database migration";
                        small { "Shown on the Uptime page and the status page." }
                    }
                }
                label.check { input type="checkbox" name="announce"; " Announce it once on the alert channels" }
                div.form-foot {
                    button.btn.primary type="submit" { "Schedule" }
                    p { "From CI: " code { "openlens-cli maintenance schedule --minutes 15 --text \"Deploy\"" } }
                }
            }
        }
    }
}
