//! The availability card of the Uptime page (#560, D-078), in the v2
//! style: one KPI tile per window and environment (what the figure
//! leaves out in its foot), the error budget card with its meter when a
//! target is set, and for admins the target form card.

use openlens_engine::maintenance::length;
use openlens_engine::slo::{Report, Slo, WindowFigure};

use super::*;

/// The card.
#[derive(Debug, Clone, Default)]
pub struct AvailabilityView {
    /// One report per environment, production first.
    pub reports: Vec<Report>,
    /// The reader may set targets.
    pub admin: bool,
}

fn window_name(label: &str) -> &'static str {
    match label {
        "24h" => "Last 24 hours",
        "7d" => "Last 7 days",
        "30d" => "Last 30 days",
        "90d" => "Last 90 days",
        "month" => "This month",
        "last_month" => "Last month",
        "365d" => "Last 365 days",
        _ => "Window",
    }
}

fn percent(a: Option<f64>) -> String {
    a.map_or_else(|| "—".into(), |a| format!("{:.3}%", a * 100.0))
}

fn burn(b: Option<f64>) -> String {
    b.map_or_else(|| "—".into(), |b| format!("{b:.1}×"))
}

/// What a figure covers, for the tile's foot.
fn covers(w: &WindowFigure) -> String {
    let mut parts = Vec::new();
    if w.unmeasured_seconds > 0 {
        parts.push(format!("not measured: {}", length(w.unmeasured_seconds)));
    }
    if w.maintenance_seconds > 0 {
        parts.push(format!(
            "maintenance left out: {}",
            length(w.maintenance_seconds)
        ));
    }
    if parts.is_empty() {
        if w.measured_seconds > 0 {
            "all of it measured".into()
        } else {
            "nothing measured yet".into()
        }
    } else {
        parts.join(" · ")
    }
}

fn tile(w: &WindowFigure) -> Markup {
    let n = w.incidents;
    html! {
        div.card.kpi {
            div.kpi-head { span { (window_name(w.label)) } }
            div.kpi-value {
                @match w.availability {
                    Some(a) => { (format!("{:.3}", a * 100.0)) small { "%" } }
                    None => { "—" }
                }
            }
            div.kpi-foot {
                span { (covers(w)) }
                span {
                    (n) (if n == 1 { " incident" } else { " incidents" })
                    @if let Some(p) = w.p95_ms { " · p95 " (p) " ms" }
                }
            }
        }
    }
}

fn budget_card(env: &str, s: &Slo) -> Markup {
    let (tone, word) = if s.spent() {
        ("bad", "Budget spent")
    } else if s.burning_fast() {
        ("warn", "Burning fast")
    } else {
        ("good", "Within budget")
    };
    // The meter is SVG (no style attribute under the CSP): spent over
    // budget, in hundredths.
    let width = if s.budget_seconds > 0 {
        (s.spent_seconds.min(s.budget_seconds) * 100 / s.budget_seconds).clamp(0, 100)
    } else {
        100
    };
    html! {
        div.card.budget {
            div.section-head {
                h3 { "Error budget · " (env) " · target " (format!("{:.3}%", s.target * 100.0)) " over 30 days" }
                span class={ "tag tone-" (tone) } { (word) }
            }
            p {
                "Now " strong { (percent(s.availability)) } ". The target allows " strong { (length(s.budget_seconds)) } " of downtime: "
                (length(s.spent_seconds)) " spent, " strong { (length(s.left_seconds)) } " left."
            }
            svg.budget-meter viewBox="0 0 100 6" preserveAspectRatio="none" role="img" aria-label=(format!("{width}% of the error budget spent")) {
                rect.track x="0" y="0" width="100" height="6" rx="3" {}
                rect class={ "fill tone-" (tone) } x="0" y="0" width=(width) height="6" rx="3" {}
            }
            p.muted.small {
                "Burn rate: last hour " (burn(s.burn_1h)) ", last day " (burn(s.burn_1d))
                ". At 1× the budget lasts exactly 30 days. An alert goes out when it is spent, and when at the last day's rate it is gone within 2 days."
            }
        }
    }
}

fn target_form(project: &str, v: &AvailabilityView) -> Markup {
    html! {
        form.card.form-card method="post" action={ "/p/" (project) "/uptime/target" } aria-labelledby="target-h" {
            div.section-head { h2 id="target-h" { "Availability target" } }
            div.form-grid {
                label.field { "Environment"
                    select name="environment" {
                        @for r in &v.reports { option value=(r.environment) { (r.environment) } }
                    }
                    small { "One target per environment." }
                }
                label.field { "Target (%)"
                    input type="number" name="target" min="50" max="99.999" step="0.001" placeholder="99.9";
                    small { "Over a rolling 30 days, from 50 to 99.999. Empty removes the target." }
                }
            }
            div.form-foot {
                button.btn.primary type="submit" { "Save" }
                p { "With a target, Uptime shows the error budget, and alerts go out when it is spent or burning fast." }
            }
        }
    }
}

/// The availability card.
pub fn availability_card(project: &str, v: &AvailabilityView) -> Markup {
    html! {
        section.availability id="availability" aria-labelledby="availability-h" {
            div.section-head {
                h2 id="availability-h" { (icon("task")) "Availability" }
                p.muted.small { "Time up over time measured. Time with no check (OpenLens stopped, no probe yet) counts neither way; planned maintenance is left out." }
            }
            @for r in &v.reports {
                h3 { (r.environment) }
                div.kpis { @for w in &r.windows { (tile(w)) } }
                @if let Some(s) = &r.slo { (budget_card(&r.environment, s)) }
            }
            @if v.admin {
                (target_form(project, v))
            } @else if v.reports.iter().all(|r| r.slo.is_none()) {
                p.muted.small { "No availability target. An admin can set one here." }
            }
        }
    }
}
