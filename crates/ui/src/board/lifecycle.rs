//! The board's Lifecycle block (#708, D-111): visitor → account created →
//! activated → paid → retained, one tile per stage the engine returned
//! (a stage without data is not there: an internal tool shows no visitor
//! and no paid tile). The numbers are those of the `lifecycle` command.

use super::{Kpi, kpi_tile};
use crate::*;
use openlens_engine::lifecycle::{Lifecycle, LifecycleLine};

/// A stage's tile label.
fn label(key: &str) -> &'static str {
    match key {
        "visitor" => "Visitors",
        "account" => "Accounts created",
        "activated" => "Activated",
        "paid" => "Paid",
        _ => "Retained",
    }
}

/// The visitor → account rate, in words: a lower bound with its upper
/// one, or why it is not available (rule 11).
fn visitor_rate(l: &LifecycleLine) -> Option<String> {
    let r = l.visitor_rate.as_ref()?;
    Some(if r.available {
        format!(
            "Visitor → account: at least {} (at most {})",
            pct(r.rate),
            pct(r.upper)
        )
    } else {
        format!(
            "Visitor → account: {}",
            r.why.as_deref().unwrap_or("not available")
        )
    })
}

/// One stage's line under its number.
fn sub(l: &LifecycleLine, key: &str, period: &str) -> String {
    let accounts = l
        .stages
        .iter()
        .find(|s| s.key == "account")
        .map_or(0, |s| s.people);
    let of = |n: i64| pct(n as f64 / accounts.max(1) as f64);
    let people = l
        .stages
        .iter()
        .find(|s| s.key == key)
        .map_or(0, |s| s.people);
    match key {
        "visitor" => format!("anonymous, {period}"),
        "account" => format!("signed up {period}"),
        "activated" => format!("{} of them reached a success event", of(people)),
        "paid" => format!("{} of them paid on a success event", of(people)),
        _ => match &l.retention {
            Some(r) if r.judged > 0 => format!(
                "of {} old enough to judge, active in the last 7 days; {} churned",
                compact(r.judged),
                compact(r.churned)
            ),
            _ => "none old enough to judge yet (older than the churn window)".to_owned(),
        },
    }
}

/// The Lifecycle block, when the window has an account or a visitor.
pub(super) fn lifecycle(b: &Board) -> Markup {
    let Some(l): Option<&Lifecycle> = b.lifecycle.as_ref() else {
        return html! {};
    };
    let period = super::period(b, "in the last 30 days");
    let line = &l.total;
    html! {
        section.card.pad.stack aria-labelledby="life-h" {
            div.section-head {
                h2 id="life-h" { "Lifecycle" }
            }
            div.kpis {
                @for s in &line.stages {
                    (kpi_tile(None, label(s.key), Kpi {
                        value: html! { (compact(s.people)) },
                        sub: sub(line, s.key, &period),
                        moved: None,
                        graded: None,
                        more: (s.key == "account").then(|| visitor_rate(line)).flatten(),
                    }, false))
                }
            }
            p.muted.small {
                "A stage shows only when the data has it. Signup: the account's "
                code { "created_at" } " when the app sends it, else the first identify. The visitor → account rate is a lower bound: a signup on another device leaves no link."
            }
        }
    }
}
