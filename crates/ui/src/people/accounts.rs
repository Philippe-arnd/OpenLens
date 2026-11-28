//! Accounts on People › Audience (#719): each account's health, at risk
//! first, and the small accounts counted together. Nothing when the app
//! sends no `account`. For an internal tool an account is a team; the
//! label stays "Accounts".

use crate::*;
use openlens_engine::accounts::{AccountState, Accounts};

/// The tone of a state's tag.
fn tone(s: AccountState) -> &'static str {
    match s {
        AccountState::AtRisk => "bad",
        AccountState::Inactive => "warn",
        AccountState::Healthy => "good",
    }
}

/// The section, or nothing when there is no account.
pub(super) fn section(x: &Accounts) -> Markup {
    if x.accounts.is_empty() && x.small.accounts == 0 {
        return html! {};
    }
    let listed = x.at_risk + x.inactive + x.healthy;
    html! {
        section.card.pad.stack aria-labelledby="acc-h" {
            div.section-head {
                h2 id="acc-h" { "Accounts" }
                span.muted.small { (listed) " listed · " (x.at_risk) " at risk · " (x.inactive) " inactive" }
            }
            @if !x.accounts.is_empty() {
                div.scroll { table.plain {
                    thead { tr {
                        th scope="col" { "Account" } th scope="col" { "State" }
                        (th_num("Users")) (th_num("Active, 30 days")) (th_num("30 days before"))
                        (th_num("Back the next week")) (th_num("Churned")) th scope="col" { "Plan" }
                    } }
                    tbody { @for a in &x.accounts {
                        tr {
                            th scope="row" { (a.account) }
                            td { span class={ "tag tone-" (tone(a.state)) } { (a.state.label()) } }
                            td.num { (compact(a.users)) }
                            td.num { (compact(a.active)) " " span.muted.small { (pct(a.active_share)) } }
                            td.num.muted { (pct(a.previous_share)) }
                            td.num { @match a.retention { Some(r) => { (pct(r)) }, None => span.muted { "—" } } }
                            td.num { (compact(a.churned)) }
                            td { (a.plan.as_deref().unwrap_or("—")) }
                        }
                    } }
                } }
            }
            @if x.small.accounts > 0 {
                p.muted.small {
                    (compact(x.small.accounts)) " small account" @if x.small.accounts != 1 { "s" }
                    " (fewer than 3 known users) counted together: " (compact(x.small.users)) " users, "
                    (compact(x.small.active)) " active in the last 30 days."
                }
            }
            @if x.truncated { p.muted.small { "Only part of the accounts could be read." } }
            p.muted.small { "Last " (x.days) " days in " (x.environment) ". At risk: the share of active users fell by a third or more against the 30 days before, or a quarter of the users churned. Inactive: no active user for 14 days." }
        }
    }
}
