//! Churn (#247, #269) in the v2 parts (#550): the lead finding when the
//! data has one, the key numbers, the charts as cards, the main tables as
//! table cards and the secondary ones as disclosures.

use super::{card, head, key_numbers, plain_tile, share_rows};
use crate::board::{Kpi, Side, barcell, compare, kpi_tile};
use crate::tables::{Col, TableCard, user_cell};
use crate::*;

/// A flag made from the users at risk, and how its arms do (#269).
pub struct Audience {
    /// Flag key.
    pub flag: String,
    /// Switched on.
    pub enabled: bool,
    /// Listed users who get it.
    pub treated: i64,
    /// Of them, active in the last 7 days.
    pub treated_active: i64,
    /// Listed users held out (control).
    pub held: i64,
    /// Of them, active in the last 7 days.
    pub held_active: i64,
}

/// What the churn page shows besides the report (#269).
pub struct ChurnExtras {
    /// Admins export and make audiences.
    pub admin: bool,
    /// The map's cancel event, if declared.
    pub churn_event: Option<String>,
    /// Answers of surveys shown on it.
    pub exit: Vec<openlens_engine::SurveyResults>,
    /// Flags made from the users at risk.
    pub audiences: Vec<Audience>,
}

/// A last place far above its usual share: churned users were last seen
/// there over half as often again as users who stayed, three users at least.
fn unusual(n: i64, share: f64, base: f64) -> bool {
    share > base * 1.5 && n >= 3
}

/// The lead finding: the first last place (the rows come largest first)
/// that churned users left from more than usual. None when no place
/// stands out, and then the page shows no lead rather than an invented one.
pub(crate) fn lead_place(c: &openlens_engine::ChurnReport) -> Option<&(String, i64, f64, f64)> {
    c.last_place
        .iter()
        .find(|(place, n, share, base)| place != "(none)" && unusual(*n, *share, *base))
}

/// The lead card: where churned users were last seen, against users who
/// stayed, and a session to watch.
fn lead(pid: &str, (place, n, share, base): &(String, i64, f64, f64)) -> Markup {
    let sides = [
        Side {
            label: "stayed".to_owned(),
            value: *base,
            shown: pct(*base),
            base: "users who stayed".to_owned(),
        },
        Side {
            label: "churned".to_owned(),
            value: *share,
            shown: pct(*share),
            base: format!("{} churned users", compact(*n)),
        },
    ];
    html! {
        article.card.lead aria-labelledby="lead-h" {
            div.lead-top {
                span.tag.tone-warn { (icon("down")) "Churn" }
                span.muted.small.mono { (place) }
            }
            div.lead-text {
                h2 id="lead-h" { "Churned users were last seen in " span.mono { (place) } }
                p.evidence { (pct(*share)) " of churned users (" (compact(*n)) ") were last active there, against " (pct(*base)) " of the users who stayed." }
            }
            (compare(&sides, true))
            div.next {
                (icon("idea"))
                div {
                    strong { "Next step" }
                    p { "Replay a session that ended there to see what stopped them, then fix that moment." }
                }
            }
            @if !shared() {
                div.actions {
                    a.btn.primary href={ "/p/" (pid) "/watch?churned_at=" (url_enc(place)) } { "Watch a session that churned here" }
                    a.btn href="#last-place" { "The last thing they did" }
                }
            }
        }
    }
}

/// A secondary section, folded.
fn disclose(title: &str, note: &str, body: Markup) -> Markup {
    html! {
        details.card.disclose {
            summary {
                h2 { (title) }
                @if !note.is_empty() { span.muted.small { (note) } }
                (icon("chev"))
            }
            div.body { (body) }
        }
    }
}

/// Churn (#247): how many leave, after how long, at which moment, with
/// what friction, and who is at risk now; why they leave and who came
/// back (#269).
pub fn churn(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    c: &openlens_engine::ChurnReport,
    x: &ChurnExtras,
) -> Markup {
    let rate = c.churned_recent as f64 / c.active_at_start.max(1) as f64;
    let net = c.new_recent + c.resurrected - c.churned_recent;
    let left = c.leave_week.iter().map(|x| x.1).sum::<i64>().max(1) as f64;
    let leave: Vec<(String, i64)> = c
        .leave_week
        .iter()
        .map(|(wk, n)| {
            let l = if *wk >= 12 {
                "week 12+".to_owned()
            } else {
                format!("week {}", wk + 1)
            };
            (l, *n)
        })
        .collect();
    // The churn page is the detail (#409): only "At risk now" opens its
    // list below; the others stay plain on purpose.
    let tiles = html! {
        (plain_tile("Churn rate", html! { (pct(rate)) }, format!("{} of {} users active {} days ago left", compact(c.churned_recent), compact(c.active_at_start), c.window)))
        (plain_tile("Net change", html! { (if net >= 0 { "+" } else { "" }) (compact(net)) }, format!("{} new + {} back − {} gone", compact(c.new_recent), compact(c.resurrected), compact(c.churned_recent))))
        (kpi_tile(Some("#at-risk"), "At risk now", Kpi {
            value: html! { (compact(c.at_risk.len() as i64)) },
            spark: html! {},
            sub: "quiet for over half the window, less active than before".to_owned(),
            moved: None,
        }, false))
        (plain_tile("Median lifetime", html! { (c.median_days.map_or_else(|| "over 90 days".into(), |d| format!("{d} days"))) }, "half the users are gone by then".to_owned()))
    };
    let pid = p.id;
    layout_in(
        "Churn",
        Some(p),
        context_bar(p.id, b, f, "/churn", false),
        html! {
            div.board.people {
                (head("Churn", html! { "A user has churned after " strong { (c.window) " days" } " without using the app (" code { "churn_after_days" } " in the feature map). Last 90 days, " strong { (f.environment) } ", logged-in users." }))
                @if let Some(s) = c.suggested.filter(|s| *s != c.window) {
                    p.notice { "Your users usually come back within " strong { (s) " days" } " (9 gaps out of 10 between active days are shorter): " code { "churn_after_days: " (s) } " would fit this app better. Nothing changes until the feature map says so." }
                }
                @if let Some(l) = lead_place(c) { (lead(pid, l)) }
                (key_numbers(tiles))
                div.split {
                    (card("ch-week", "Churned per week", html! {}, html! {
                        @if c.by_week.is_empty() { p.muted.small { "Nobody has churned in the window." } }
                        @else { (sparkline(&c.by_week)) p.muted.small { "Users whose last active day fell that week." } }
                    }))
                    (card("ch-stay", "How long they stay", html! {}, html! {
                        div.scroll { table.share-bars { tbody { @for (d, s) in &c.survival {
                            tr { th scope="row" { "day " (d) } td.bar { (barcell(*s)) } }
                        } } } }
                        p.muted.small { "Share still active N days after their first visit (Kaplan–Meier: users who joined recently count only for the days they could have stayed)." }
                    }))
                    (card("ch-leave", "When they leave", html! {}, html! {
                        @if leave.is_empty() { p.muted.small { "Nobody has churned yet." } }
                        @else {
                            (share_rows(&leave, left))
                            p.muted.small { "Week of life in which churned users were last active, as a share of them. A spike in week 1 is onboarding; at the end of a trial, the trial." }
                        }
                    }))
                    (card("ch-friction", "Friction before leaving", html! {}, html! {
                        p { "On their last active day, " strong { (pct(c.errors_at_end.0)) } " of churned users hit an error, against " (pct(c.errors_at_end.1)) " of users who stayed." }
                        @if !c.friction.is_empty() {
                            div.scroll { table {
                                thead { tr { th scope="col" { "On the last day" } (th_num("Churned")) (th_num("Stayed")) } }
                                tbody { @for (k, ch, st) in &c.friction {
                                    tr { th scope="row" { (k) } td.num { strong { (pct(*ch)) } } td.num { (pct(*st)) } }
                                } }
                            } }
                        }
                        p.small { a href={ "/p/" (pid) "/errors" } { "Errors →" } " · " a href={ "/p/" (pid) "/flows" } { "Feature flow (exits) →" } }
                    }))
                }
                section.reasons aria-labelledby="reasons-h" {
                    h2.eyebrow id="reasons-h" { "Top reasons" @if !c.reasons.is_empty() { " " span.count { (c.reasons.len()) } } }
                    @if c.reasons.is_empty() {
                        div.card.pad { p.muted { "No clear reason yet: each comparison needs at least 8 users on both sides, and a gap of 5 points or more." } }
                    } @else {
                        div.card.ranked {
                            ol { @for (i, r) in c.reasons.iter().enumerate() {
                                li.row {
                                    span.rank aria-hidden="true" { (format!("{:02}", i + 1)) }
                                    div { h3 { (r.title) } div.meta { span { (r.evidence) } } }
                                }
                            } }
                        }
                        p.muted.small { "Ranked by the gap between users who left and users who stayed. These are correlations: a feature people try early may be a sign of fit rather than its cause. Replay a churned session at the place named to check." }
                    }
                }
                div #last-place {
                    @if c.last_place.is_empty() {
                        (card("lp-h", "The last thing they did", html! {}, html! { p.muted { "Nobody has churned in the window." } }))
                    } @else {
                        (TableCard {
                            label: "The last thing they did",
                            head: html! { h2 { "The last thing they did" } },
                            cols: &[Col::Shown("Feature or page"), Col::Num("Churned users"), Col::Num("Share"), Col::Num("Users who stayed"), Col::Hidden("Flag")],
                            body: html! { @for (place, n, share, base) in &c.last_place {
                                tr {
                                    th scope="row" { span.mono { (place) } }
                                    td.num { (compact(*n)) }
                                    td.num { strong { (pct(*share)) } }
                                    td.num { (pct(*base)) }
                                    td { @if unusual(*n, *share, *base) { span.tag.tone-bad { "more than usual" } } }
                                }
                            } },
                            foot: html! { div.pad { p.muted.small { "Where churned users were last active, against where users who stayed were last seen. A place far above its usual share is where to look (and replay)." } } },
                        }.render())
                    }
                }
                (at_risk(pid, c, x))
                @if !x.audiences.is_empty() {
                    (TableCard {
                        label: "Win-back flags",
                        head: html! { h2 { "Win-back flags" } },
                        cols: &[Col::Shown("Flag"), Col::Num("Nudged"), Col::Num("Active, last 7 days"), Col::Num("Held out"), Col::Num("Active, last 7 days")],
                        body: html! { @for a in &x.audiences {
                            tr {
                                th scope="row" { a href={ "/p/" (pid) "/flags-edit?key=" (a.flag) } { code { (a.flag) } } @if !a.enabled { " " span.tag { "off" } } }
                                td.num { (a.treated) } td.num { strong { (pct(a.treated_active as f64 / a.treated.max(1) as f64)) } }
                                td.num { (a.held) } td.num { (pct(a.held_active as f64 / a.held.max(1) as f64)) }
                            }
                        } },
                        foot: html! {},
                    }.render())
                }
                (secondary(c, x))
            }
        },
    )
}

/// Who is at risk now: a table card, with the exports and the win-back
/// flag form for admins.
fn at_risk(pid: &str, c: &openlens_engine::ChurnReport, x: &ChurnExtras) -> Markup {
    if c.at_risk.is_empty() {
        return html! {
            div #at-risk { (card("risk-h", "At risk now", html! {}, html! { p.muted { "Nobody: active users keep their pace." } })) }
        };
    }
    let foot = html! {
        div.pad.stack {
            p.muted.small { "Open a profile to see their last sessions and replays. Win them back before the window closes." }
            @if x.admin {
                div.actions {
                    a.btn href={ "/p/" (pid) "/churn/at-risk.csv" } { "CSV" }
                    a.btn href={ "/p/" (pid) "/churn/at-risk.json" } { "JSON" }
                }
                form.inline method="post" action={ "/p/" (pid) "/churn/audience" } {
                    label { "Make a flag for them, holding out " input.narrow type="number" name="holdout" min="0" max="50" value="20"; " % as a control group " }
                    button.btn type="submit" { "Make the flag" }
                }
                p.muted.small { "The flag starts off and lists these users (frozen now). Put the nudge behind " code { "openlens.isEnabled(key)" } " and switch it on; the held-out users never get it, so you can compare." }
            }
        }
    };
    html! {
        div #at-risk {
            (TableCard {
                label: "At risk now",
                head: html! { h2 { "At risk now" } span.muted.small { (c.at_risk.len()) " users" } },
                cols: &[Col::Shown("User"), Col::Shown("Role"), Col::Shown("Last active"), Col::Num("Quiet for"), Col::Num("Active days, last window"), Col::Num("Window before")],
                body: html! { @for r in &c.at_risk {
                    tr {
                        td { (user_cell(pid, &r.distinct_id)) }
                        td.muted { (r.role.as_deref().unwrap_or("—")) }
                        td.when { (r.last_day) }
                        td.num { (r.idle_days) " days" }
                        td.num { (r.recent_days) }
                        td.num { (r.before_days) }
                    }
                } },
                foot,
            }.render())
        }
    }
}

/// The secondary sections, folded: early features, arrival release,
/// role, who came back and the exit survey.
fn secondary(c: &openlens_engine::ChurnReport, x: &ChurnExtras) -> Markup {
    html! {
        @if !c.lift.is_empty() {
            (disclose("Features tried in the first 14 days", &format!("{} features", c.lift.len()), html! {
                div.scroll { table {
                    thead { tr { th scope="col" { "Feature" } (th_num("Tried it")) (th_num("Churned")) (th_num("Did not")) (th_num("Churned")) } }
                    tbody { @for l in &c.lift {
                        tr {
                            th scope="row" { code { (l.feature) } }
                            td.num { (compact(l.adopters)) } td.num { strong { (pct(l.churn_adopters)) } }
                            td.num { (compact(l.non_adopters)) } td.num { (pct(l.churn_non)) }
                        }
                    } }
                } }
                p.muted.small { "Only users first seen 14 days ago or more, so everyone had the same time to try each feature (a user who stays longer has more time to try anything)." }
            }))
        }
        @if !c.by_release.is_empty() {
            (disclose("By arrival release", "", html! {
                div.scroll { table {
                    thead { tr { th scope="col" { "Release" } (th_num("Users")) (th_num("Churned")) } }
                    tbody { @for (r, n, s) in &c.by_release { tr { th scope="row" { code { (r) } } td.num { (compact(*n)) } td.num { (pct(*s)) } } } }
                } }
            }))
        }
        @if !c.by_role.is_empty() {
            (disclose("By role", "", html! {
                div.scroll { table {
                    thead { tr { th scope="col" { "Role" } (th_num("Users")) (th_num("Still here at 30 days")) } }
                    tbody { @for (r, n, s) in &c.by_role { tr { th scope="row" { (r) } td.num { (compact(*n)) } td.num { (pct(*s)) } } } }
                } }
            }))
        }
        (disclose("Who came back", &if c.winback.users == 0 { "nobody, last 30 days".to_owned() } else { format!("{} users, last 30 days", c.winback.users) }, html! {
            @if c.winback.users == 0 { p.muted { "Nobody came back after churning in the last 30 days." } }
            @else {
                p { strong { (c.winback.users) } " users came back in the last 30 days, active " (format!("{:.1}", c.winback.mean_active_days)) " days since on average." }
                p.small { "On release: " @for (i, (r, n)) in c.winback.by_release.iter().enumerate() { @if i > 0 { ", " } code { (r) } " (" (n) ")" } }
                p.small { "First used: " @for (i, (r, n)) in c.winback.by_feature.iter().enumerate() { @if i > 0 { ", " } code { (r) } " (" (n) ")" } }
            }
        }))
        (disclose("Exit survey", if x.churn_event.is_none() { "no cancel event declared" } else { "" }, html! {
            @match &x.churn_event {
                None => p.muted { "Declare the app's cancel event in the feature map (" code { "churn_event: subscription_cancelled" } "): users who send it count as churned at once, and a survey triggered on it becomes the exit survey shown here." },
                Some(ev) => {
                    p.small { code { (ev) } " · " (c.cancelled) " users cancelled in the window." }
                    @if x.exit.is_empty() { p.muted { "No survey is triggered on " code { (ev) } " yet: add one on the Surveys page to ask why they leave." } }
                    @for s in &x.exit {
                        p { strong { (s.key) } }
                        ul.plain-list.small {
                            @for (a, n) in &s.answers { li { (a) " · " (n) } }
                            @for t in &s.latest_text { li.muted { "“" (t) "”" } }
                        }
                    }
                }
            }
        }))
    }
}
