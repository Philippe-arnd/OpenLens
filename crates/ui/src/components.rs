//! Shared page parts: the context bar, tiles, sparklines, bars, release comparison, insight cards, the empty state.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// Which part of the context bar a form sets (#542): its own field is
/// left out of the hidden ones the form carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctx {
    /// The board's window (`since`, `from`, `to`).
    Period,
    /// `env`.
    Env,
    /// `role`.
    Role,
    /// `release`.
    Release,
    /// `internal` (the team toggle).
    Team,
}

/// A page's own path and query (`/breakdown?by=role`): a GET form drops
/// the query of its action, so the pairs ride along as hidden fields. The
/// pages put fixed, plain values there.
fn split_action(action: &str) -> (&str, Vec<(&str, &str)>) {
    match action.split_once('?') {
        Some((path, q)) => (
            path,
            q.split('&')
                .filter(|kv| !kv.is_empty())
                .map(|kv| kv.split_once('=').unwrap_or((kv, "")))
                .collect(),
        ),
        None => (action, Vec::new()),
    }
}

/// The page's query and every current filter but `own`, as the
/// `(name, value)` pairs of the URL (D-060: the window too), so applying
/// one chip keeps the others.
pub(crate) fn kept_filters(f: &Filters, own: Ctx, page: &[(&str, &str)]) -> Vec<(String, String)> {
    use openlens_engine::Window;
    let mut v: Vec<(String, String)> = page
        .iter()
        .map(|&(k, val)| (k.to_owned(), val.to_owned()))
        .collect();
    if own != Ctx::Env {
        v.push(("env".into(), f.environment.clone()));
    }
    if let Some(r) = f.role.as_ref().filter(|_| own != Ctx::Role) {
        v.push(("role".into(), r.clone()));
    }
    if let Some(r) = f.release.as_ref().filter(|_| own != Ctx::Release) {
        v.push(("release".into(), r.clone()));
    }
    if f.internal && own != Ctx::Team {
        v.push(("internal".into(), "1".into()));
    }
    if own != Ctx::Period {
        match &f.window {
            Window::Since { release } => v.push(("since".into(), release.clone())),
            Window::Range { from, to } => {
                v.push(("from".into(), from.to_string()));
                v.push(("to".into(), to.to_string()));
            }
            Window::Fixed | Window::Release => {}
        }
    }
    v
}

fn hidden_fields(pairs: &[(String, String)]) -> Markup {
    html! { @for (k, v) in pairs { input type="hidden" name=(k) value=(v); } }
}

/// One chip and its popover form (#542). The ids pair them for the anchor
/// positioning in CSS (`#ctx-env-btn` anchors `#ctx-env`).
fn chip_pop(
    id: &str,
    title: &str,
    label: Markup,
    action: &str,
    body: Markup,
    keep: &[(String, String)],
    foot: &str,
) -> Markup {
    html! {
        button.chip type="button" id={ "ctx-" (id) "-btn" } popovertarget={ "ctx-" (id) } aria-haspopup="dialog" {
            span.sr-only { (title) ": " } (label) span.chev aria-hidden="true" { "▾" }
        }
        div.pop popover="auto" id={ "ctx-" (id) } {
            form method="get" action=(action) aria-label=(title) {
                h3 { (title) }
                (body)
                (hidden_fields(keep))
                div.pop-foot { span { (foot) } button.primary.sm type="submit" { "Apply" } }
            }
        }
    }
}

/// The context bar (#542): one chip per filter (the period on the board,
/// the environment, the role, the release) whose label is the current
/// value and whose popover form applies it with the others kept, and the
/// team toggle as a link. `action` is the page's path after `/p/{project}`,
/// with its own query if it has one. Nothing on a share link.
pub(crate) fn context_bar(
    project: &str,
    b: &Board,
    f: &Filters,
    action: &str,
    with_period: bool,
) -> Markup {
    if shared() {
        return html! {};
    }
    let mut envs = b.environments.clone();
    if !envs.iter().any(|e| e == "production") {
        envs.insert(0, "production".into());
    }
    let (path, page) = split_action(action);
    let to = format!("/p/{project}{path}");
    let mut team = kept_filters(f, Ctx::Team, &page);
    if !f.internal {
        team.push(("internal".into(), "1".into()));
    }
    let team_href = format!(
        "{to}?{}",
        team.iter()
            .map(|(k, v)| format!("{}={}", url_enc(k), url_enc(v)))
            .collect::<Vec<_>>()
            .join("&")
    );
    let role = f.role.as_deref().unwrap_or("All roles");
    let release = f.release.as_deref().unwrap_or("All releases");
    html! {
        div.context role="group" aria-label="Filters" {
            @if with_period { (period_chip(&to, b, f, &page)) }
            (chip_pop("env", "Environment", html! { (icon("globe")) (f.environment) }, &to, html! {
                label.field { "Environment"
                    select name="env" {
                        @for e in &envs { option value=(e) selected[*e == f.environment] { (e) } }
                    }
                }
            }, &kept_filters(f, Ctx::Env, &page), ""))
            (chip_pop("role", "Role", html! { (role) }, &to, html! {
                label.field { "Role"
                    select name="role" {
                        option value="" { "All roles" }
                        @for r in b.roles.iter().filter_map(|r| r.role.as_deref()) {
                            option value=(r) selected[f.role.as_deref() == Some(r)] { (r) }
                        }
                    }
                }
            }, &kept_filters(f, Ctx::Role, &page), ""))
            (chip_pop("release", "Release", html! { (icon("release")) (release) }, &to, html! {
                label.field { "Release"
                    select name="release" {
                        option value="" { "All releases" }
                        @for r in &b.releases {
                            option value=(r.release) selected[f.release.as_deref() == Some(r.release.as_str())] { (r.release) }
                        }
                    }
                }
            }, &kept_filters(f, Ctx::Release, &page), ""))
            a.chip.is-off[!f.internal] href=(team_href)
                title=(if f.internal { "The team's own users are counted: leave them out" } else { "The team's own users are left out: count them" }) {
                (if f.internal { "Team included" } else { "Team excluded" })
            }
        }
    }
}

/// The period chip (#513, D-060, #542): since a release of the
/// environment, as a segmented control, or a date range, in a GET form so
/// the URL can be shared.
fn period_chip(to: &str, b: &Board, f: &Filters, page: &[(&str, &str)]) -> Markup {
    use openlens_engine::Window;
    // The environment's releases (`releases.list`), or the board's latest
    // ones when the window was not resolved (a board filtered on a release).
    let releases: Vec<&str> = match b.window.as_ref().filter(|w| !w.releases.is_empty()) {
        Some(w) => w.releases.iter().map(String::as_str).collect(),
        None => b.releases.iter().map(|r| r.release.as_str()).collect(),
    };
    let (since, from, to_day) = match &f.window {
        Window::Since { release } => (Some(release.as_str()), None, None),
        Window::Range { from, to } => (None, Some(from.to_string()), Some(to.to_string())),
        Window::Fixed | Window::Release => (None, None, None),
    };
    let days = |d: i64| {
        if d == 1 {
            "1 day".to_owned()
        } else {
            format!("{d} days")
        }
    };
    let (label, length) = match (&f.window, &b.window) {
        (Window::Range { from, to }, w) => {
            (format!("{from} – {to}"), w.as_ref().map(|w| days(w.days)))
        }
        (_, Some(w)) => match &w.release {
            Some(r) => (format!("Since {r}"), Some(days(w.days))),
            None => (w.label.clone(), None),
        },
        (Window::Since { release }, None) => (format!("Since {release}"), None),
        _ => ("Current release".to_owned(), None),
    };
    let foot = b
        .window
        .as_ref()
        .map(|w| format!("Compared with the {} before", days(w.days)))
        .unwrap_or_default();
    chip_pop(
        "period",
        "Period",
        html! { (label) @if let Some(l) = &length { " " span { "· " (l) } } },
        to,
        html! {
            fieldset.seg {
                legend.sr-only { "Release window" }
                label { input type="radio" name="since" value="" checked[since.is_none()]; "Current release" }
                @for r in &releases {
                    label { input type="radio" name="since" value=(r) checked[since == Some(*r)]; "Since " (r) }
                }
            }
            h3 { "Or a range" }
            div.range {
                // The browser shows a date field in its own locale; the hint
                // gives the format the rest of the UI prints (#557).
                label.field { "From" input type="date" name="from" value=[from] aria-describedby="period-date-format"; }
                label.field { "To" input type="date" name="to" value=[to_day] aria-describedby="period-date-format"; }
            }
            p.muted.small #period-date-format { "Dates as YYYY-MM-DD" }
            p.muted.small { "Filled dates win over the release. At most 400 days; an empty end is today." }
        },
        &kept_filters(f, Ctx::Period, page),
        &foot,
    )
}

pub(crate) fn filter_fields(envs: &[String], b: &Board, f: &Filters) -> Markup {
    html! {
            label { "Environment "
                select name="env" {
                    @for e in envs { option value=(e) selected[*e == f.environment] { (e) } }
                }
            }
            label { "Role "
                select name="role" {
                    option value="" { "All roles" }
                    @for r in b.roles.iter().filter_map(|r| r.role.as_deref()) {
                        option value=(r) selected[f.role.as_deref() == Some(r)] { (r) }
                    }
                }
            }
            label { "Release "
                select name="release" {
                    option value="" { "All releases" }
                    @for r in &b.releases {
                        option value=(r.release) selected[f.release.as_deref() == Some(r.release.as_str())] { (r.release) }
                    }
                }
            }
            label.check { input type="checkbox" name="internal" value="1" checked[f.internal]; " Include the team" }
    }
}

pub(crate) fn tile(accent: &str, glyph: &str, label: &str, value: &str, hint: &str) -> Markup {
    html! {
        div class={ "tile " (accent) } {
            div.label { span.chip-icon { (icon(glyph)) } (label) (crate::glossary::info(label)) }
            div.value { (value) }
            div.hint { (hint) }
        }
    }
}

/// One step of a project's setup (#403).
#[derive(Debug, Clone)]
pub struct SetupStep {
    /// What to do.
    pub label: &'static str,
    /// It is done.
    pub done: bool,
    /// It counts towards "Set up" (the others are shown, not required).
    pub required: bool,
    /// Where it is fixed.
    pub href: String,
}

/// Where a board card or tile leads (#409): the page, with the board's
/// filters (environment, role, release, the team toggle) carried along.
pub(crate) fn detail_url(project: &str, path: &str, f: &Filters) -> String {
    let mut q = vec![format!("env={}", url_enc(&f.environment))];
    if let Some(r) = &f.role {
        q.push(format!("role={}", url_enc(r)));
    }
    if let Some(r) = &f.release {
        q.push(format!("release={}", url_enc(r)));
    }
    if f.internal {
        q.push("internal=1".to_owned());
    }
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("/p/{project}{path}{sep}{}", q.join("&"))
}

/// A row's own link inside a linked card (#409): its page, filters kept;
/// plain text on a share link. Inner links sit above the card's stretched
/// link (CSS), so both work.
pub(crate) fn row_link(href: &str, label: &str) -> Markup {
    html! { @if crate::shared() { (label) } @else { a href=(href) { (label) } } }
}

/// A feature name that opens its page (#411), when it is in the map.
pub(crate) fn feature_link(
    project: &str,
    key: &str,
    name: &str,
    declared: bool,
    f: &Filters,
) -> Markup {
    if declared {
        row_link(
            &detail_url(project, &format!("/features/{}", url_enc(key)), f),
            name,
        )
    } else {
        html! { (name) }
    }
}

/// A card title that opens its detail (#409): the link stretches over the
/// whole card (CSS `.stretch::after`), one tab stop per card. Plain text
/// on a share link, whose detail pages need a login.
pub(crate) fn to(href: &str, label: &str) -> Markup {
    html! {
        @if crate::shared() { (label) } @else {
            a.stretch href=(href) { (label) span.details aria-hidden="true" { "Details →" } }
        }
    }
}

/// [`tile`] that opens its detail page (#409).
pub(crate) fn tile_to(
    href: &str,
    accent: &str,
    glyph: &str,
    label: &str,
    value: &str,
    hint: &str,
) -> Markup {
    html! {
        div class={ "tile " (accent) } {
            div.label { span.chip-icon { (icon(glyph)) } (to(href, label)) (crate::glossary::info(label)) }
            div.value { (value) }
            div.hint { (hint) }
        }
    }
}

/// A row of tabs (#402, reused by the feature pages #411): plain links,
/// the current one with `aria-current="page"`, so they work with the
/// keyboard and without script, and wrap on narrow screens.
/// `items` are (href, label, current).
pub(crate) fn tabs(aria_label: &str, items: &[(String, &str, bool)]) -> Markup {
    html! {
        nav.section-tabs aria-label=(aria_label) {
            @for (href, label, current) in items {
                a href=(href) aria-current=[current.then_some("page")] { (label) }
            }
        }
    }
}

/// Every IANA time zone, as `<datalist id="tz-all">` (#403): the zone
/// fields suggest them instead of taking free text blindly.
pub(crate) fn zone_list() -> Markup {
    html! {
        datalist id="tz-all" {
            @for z in chrono_tz::TZ_VARIANTS.iter() { option value=(z.name()) {} }
        }
    }
}

/// An hour picker (`""` = not set), for quiet hours (#403).
pub(crate) fn hour_select(name: &str, value: Option<u8>) -> Markup {
    html! {
        select name=(name) {
            option value="" selected[value.is_none()] { "—" }
            @for h in 0u8..24 {
                option value=(h) selected[value == Some(h)] { (format!("{h:02}:00")) }
            }
        }
    }
}

/// One entry of the timeline of changes (#267).
#[derive(Debug, Clone)]
pub struct Mark {
    /// Day (YYYY-MM-DD).
    pub day: String,
    /// `release`, `change`, `alert` or `note`.
    pub kind: String,
    /// What happened.
    pub text: String,
    /// Note id (notes only).
    pub id: Option<i64>,
    /// Who (changes and notes).
    pub by: Option<String>,
    /// The note author's id (to allow deleting).
    pub author_id: Option<String>,
}

pub(crate) fn mark_icon(kind: &str) -> &'static str {
    match kind {
        "release" => "release",
        "alert" => "alert",
        "note" => "idea",
        _ => "flag",
    }
}

/// The active users bars with the timeline's days marked under them.
pub(crate) fn sparkline_marked(series: &[(String, i64)], marks: &[Mark]) -> Markup {
    let n = series.len().max(1) as f64;
    let w = 280.0;
    let gap = 2.0;
    let bw = (w - gap * (n - 1.0)) / n;
    let on: Vec<(usize, Vec<&Mark>)> = series
        .iter()
        .enumerate()
        .map(|(i, (d, _))| (i, marks.iter().filter(|m| &m.day == d).collect::<Vec<_>>()))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    html! {
        (sparkline(series))
        @if !on.is_empty() {
            svg.marks viewBox={ "0 0 " (w) " 12" } role="img" aria-label="Changes on these days (the timeline lists them)" preserveAspectRatio="none" {
                @for (i, ms) in &on {
                    @let kind = if ms.iter().any(|m| m.kind == "alert") { "alert" } else if ms.iter().any(|m| m.kind == "release") { "release" } else { "change" };
                    circle class={ "mk " (kind) } cx=(format!("{:.1}", *i as f64 * (bw + gap) + bw / 2.0)) cy="6" r="4" {
                        title { @for (j, m) in ms.iter().enumerate() { @if j > 0 { " · " } (m.day) ": " (m.text) } }
                    }
                }
            }
        }
    }
}

pub(crate) fn sparkline(series: &[(String, i64)]) -> Markup {
    let max = series.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1) as f64;
    let n = series.len().max(1) as f64;
    let w = 280.0;
    let h = 56.0;
    let gap = 2.0;
    let bw = (w - gap * (n - 1.0)) / n;
    html! {
        svg.spark viewBox={ "0 0 " (w) " " (h) } role="img" aria-label="Daily active users, last 14 days" preserveAspectRatio="none" {
            @for (i, (date, v)) in series.iter().enumerate() {
                @let bh = ((*v as f64) / max * (h - 4.0)).max(2.0);
                @let last = i + 1 == series.len();
                rect class=(if last { "bar now" } else { "bar" })
                    x=(i as f64 * (bw + gap)) y=(h - bh) width=(bw) height=(bh) rx="2" {
                    title { (date) ": " (compact(*v)) " users" }
                }
            }
        }
    }
}

pub(crate) fn status(rating: &str) -> Markup {
    let (cls, glyph, text) = match rating {
        "good" => ("good", "task", "good"),
        "needs-improvement" => ("warn", "alert", "needs work"),
        "poor" => ("bad", "ban", "poor"),
        _ => ("muted", "alert", "unknown"),
    };
    html! { span class={ "status " (cls) } { (self::icon(glyph)) (text) } }
}

/// A horizontal meter filled to `frac` (0..1). Drawn as SVG so the length
/// is an attribute, not an inline style: the dashboard CSP forbids those.
pub(crate) fn meter(frac: f64) -> Markup {
    let w = format!("{:.1}%", (frac.clamp(0.0, 1.0) * 100.0).max(1.5));
    html! {
        svg.fill aria-hidden="true" focusable="false" { rect width=(w) height="6" rx="3" {} }
    }
}

pub(crate) fn bars(rows: &[(String, i64, String)]) -> Markup {
    let max = rows.iter().map(|r| r.1).max().unwrap_or(0).max(1) as f64;
    html! {
        table.bars {
            tbody {
                @for (name, v, extra) in rows {
                    tr {
                        th scope="row" { (name) }
                        td.meter {
                            (meter((*v as f64) / max))
                        }
                        td.num { (compact(*v)) }
                        td.num.muted { (extra) }
                    }
                }
            }
        }
    }
}

/// One compared metric: label, new value, old value, whether higher is
/// better, and how to format it.
pub(crate) type Compared = (&'static str, f64, f64, bool, fn(f64) -> String);

/// "Latest vs previous release" chips: arrow, change, and whether the change
/// is good (green) or bad (red) for that metric — never colour alone.
pub(crate) fn release_compare(rel: &[openlens_engine::ReleaseRow]) -> Markup {
    let [new, old, ..] = rel else { return html! {} };
    if new.sessions < 8 || old.sessions < 8 {
        return html! { p.muted.small { "Comparison appears once both of the latest releases have at least 8 sessions." } };
    }
    // (label, new, old, higher_is_better, formatter)
    let rows: [Compared; 4] = [
        ("Avg session", new.avg_secs, old.avg_secs, true, duration),
        (
            "Tasks / session",
            new.tasks_per_session,
            old.tasks_per_session,
            true,
            |v| format!("{v:.2}"),
        ),
        ("Bounce", new.bounce_rate, old.bounce_rate, false, pct),
        (
            "Sessions with errors",
            new.error_rate,
            old.error_rate,
            false,
            pct,
        ),
    ];
    html! {
        p.small { strong { (new.release) } " compared with " strong { (old.release) } ":" }
        ul.deltas {
            @for (label, n, o, up_good, fmt) in rows {
                @let change = if o.abs() > f64::EPSILON { (n - o) / o } else { 0.0 };
                @let flat = change.abs() < 0.05;
                @let good = (n > o) == up_good;
                @let (cls, arrow, word) = if flat { ("flat", "release", "about the same") } else if good { ("up", if n > o { "up" } else { "down" }, "better") } else { ("down", if n > o { "up" } else { "down" }, "worse") };
                li class=(cls) {
                    span.label { (label) }
                    " " strong { (fmt(n)) } " "
                    span.change { (icon(arrow)) (format!("{:+.0}%", change * 100.0)) " · " (word) }
                    span.muted { " (was " (fmt(o)) ")" }
                }
            }
        }
    }
}

pub(crate) fn insight_cards(pid: &str, b: &Board, plain: &[(String, String)]) -> Markup {
    let accent = |kind: &str| match kind {
        "stick_driver" => ("mint", "star"),
        "value_moment" => ("mint", "spark"),
        "flag_win" => ("mint", "up"),
        "churn_point" => ("coral", "down"),
        "friction" | "funnel_drop" => ("coral", "funnel"),
        "role_blind_spot" => ("violet", "shield"),
        "release_regression" | "flag_regression" => ("sun", "alert"),
        _ => ("sky", "bolt"),
    };

    html! {
        section.insights aria-label="What to focus on" {
            h2.violet { (icon("idea")) "What to focus on" }
            @if b.insights.is_empty() {
                div.insight.waiting {
                    @if b.readiness.judged_users < b.readiness.needed {
                        p { strong { "Insights need a little history." } " They compare users who stayed with users who left, so they wait until "
                            (b.readiness.needed) " users are older than the churn window (now " (b.readiness.judged_users) ")." }
                        p.muted.small { "Backfilling past users and success events (" code { "openlens-cli import" } ") gets there on day one." }
                    } @else {
                        p { strong { "Nothing stands out right now." } " No feature, role, release or page differs enough from the rest to be worth your attention." }
                    }
                }
            } @else {
                ol.cards {
                    @for (i, c) in b.insights.iter().enumerate() {
                        @let (cls, glyph) = accent(c.kind);
                        li class={ "insight " (cls) } {
                            span.rank aria-hidden="true" { (icon(glyph)) }
                            div {
                                @let pl = plain.get(i).filter(|x| x.0 != c.title || x.1 != c.next_step);
                                @if let Some((t, _)) = pl {
                                    h3 title={ "Original: " (c.title) } { span.sr-only { "Insight " (i + 1) ": " } (t) }
                                } @else {
                                    h3 { span.sr-only { "Insight " (i + 1) ": " } (c.title) }
                                }
                                p.evidence { (c.evidence) }
                                p.next title=[pl.map(|_| format!("Original: {}", c.next_step))] { span.label { "Next step" } " " (pl.map_or(c.next_step.as_str(), |x| x.1.as_str())) }
                                @if c.kind == "churn_point" {
                                    @if !shared() { p { a.button href={ "/p/" (pid) "/watch?churned_at=" (url_enc(&c.subject)) } { (icon("play")) "Watch a session that churned here" } } }
                                }
                                @if !shared() { p.small { a href={ "/p/" (pid) "/tickets/new?kind=insight&subject=" (url_enc(&format!("{}:{}", c.kind, c.subject))) } { "Create ticket →" } } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Empty state: no events yet. Shows exactly what the agent (or a human) must add.
pub(crate) fn empty(p: &ProjectInfo<'_>, host: &str) -> Markup {
    let tag = format!(
        r#"<script src="{host}/c.js" data-key="{}" defer></script>"#,
        p.key
    );
    let ask = format!(
        "Connect this app to OpenLens. Follow {host}/agents.md — project key {}.",
        p.key
    );
    html! {
        section.empty {
            div.blob aria-hidden="true" {}
            h1 { "Your board is warming up" }
            p { "No events yet for " strong { (p.name) } ". Nothing to configure here: your coding agent wires the app, drafts the feature map and backfills history." }
            h2 { "Ask your agent" }
            pre.copy { (ask) }
            h2 { "Or add one tag yourself" }
            pre.copy { (tag) }
            p.muted { "Then call " code { "openlens.identify(user.id, { role })" } " once the user is known. This page fills in as soon as the first events land." }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The popover form labelled `label` in a rendered context bar.
    pub(crate) fn pop_form(html: &str, label: &str) -> String {
        let at = format!(r#"aria-label="{label}""#);
        html.split("<form ")
            .skip(1)
            .find(|s| s.contains(&at))
            .and_then(|s| s.split("</form>").next())
            .unwrap_or_default()
            .to_owned()
    }

    fn filters() -> Filters {
        let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        Filters {
            environment: "staging".into(),
            role: Some("admin".into()),
            release: Some("v2".into()),
            internal: true,
            window: openlens_engine::Window::Range {
                from: day("2026-10-01"),
                to: day("2026-10-31"),
            },
            ..Filters::default()
        }
    }

    #[test]
    fn applying_one_chip_keeps_the_other_filters_in_the_url() {
        let f = filters();
        let html =
            context_bar("p1", &Board::default(), &f, "/breakdown?by=role", true).into_string();
        // Each chip's form: its own field(s), and every other active filter
        // (and the page's own query) as hidden inputs.
        let all = [
            ("by", "role"),
            ("env", "staging"),
            ("role", "admin"),
            ("release", "v2"),
            ("internal", "1"),
            ("from", "2026-10-01"),
            ("to", "2026-10-31"),
        ];
        for (label, own) in [
            ("Period", &["from", "to"][..]),
            ("Environment", &["env"][..]),
            ("Role", &["role"][..]),
            ("Release", &["release"][..]),
        ] {
            let form = pop_form(&html, label);
            assert!(
                form.contains(r#"action="/p/p1/breakdown""#),
                "{label}: {form}"
            );
            for (k, v) in all {
                let hidden = format!(r#"type="hidden" name="{k}" value="{v}""#);
                assert_eq!(
                    form.contains(&hidden),
                    !own.contains(&k),
                    "{label} / {k}: {form}"
                );
            }
        }
        assert!(!html.contains("style="), "CSP: {html}");
        assert!(!html.contains("<script"), "{html}");
    }

    #[test]
    fn the_team_chip_toggles_the_team_and_keeps_the_others() {
        let href = |html: &str| {
            html.split("<a ")
                .nth(1)
                .and_then(|a| a.split(r#"href=""#).nth(1))
                .and_then(|h| h.split('"').next())
                .unwrap_or_default()
                .replace("&amp;", "&")
        };
        let on = filters();
        let off = Filters {
            internal: false,
            ..filters()
        };
        let b = Board::default();
        let (on_html, off_html) = (
            context_bar("p1", &b, &on, "/audience", false).into_string(),
            context_bar("p1", &b, &off, "/audience", false).into_string(),
        );
        let (from_on, from_off) = (href(&on_html), href(&off_html));
        assert!(!from_on.contains("internal="), "{from_on}");
        assert!(from_off.contains("internal=1"), "{from_off}");
        for link in [&from_on, &from_off] {
            assert!(link.starts_with("/p/p1/audience?"), "{link}");
            for kept in [
                "env=staging",
                "role=admin",
                "release=v2",
                "from=2026-10-01",
                "to=2026-10-31",
            ] {
                assert!(link.contains(kept), "{kept}: {link}");
            }
        }
        // Dashed when the team is left out.
        assert!(off_html.contains(r#"class="chip is-off""#), "{off_html}");
        assert!(!on_html.contains("is-off"), "{on_html}");
        // Off the board there is no period chip, but the window rides along.
        assert!(pop_form(&on_html, "Period").is_empty(), "{on_html}");
        assert!(
            pop_form(&on_html, "Role").contains(r#"name="from" value="2026-10-01""#),
            "{on_html}"
        );
    }

    #[test]
    fn every_chip_is_anchored_to_its_popover_in_the_stylesheet() {
        let html = context_bar("p1", &Board::default(), &filters(), "", true).into_string();
        for id in ["period", "env", "role", "release"] {
            assert!(
                html.contains(&format!(r#"popovertarget="ctx-{id}""#)),
                "{id}: {html}"
            );
            assert!(
                html.contains(&format!(r#"id="ctx-{id}-btn""#)),
                "{id}: {html}"
            );
            assert!(html.contains(&format!(r#"id="ctx-{id}""#)), "{id}: {html}");
            assert!(
                crate::CSS.contains(&format!("#ctx-{id}-btn {{ anchor-name: --ctx-{id}; }}")),
                "{id}"
            );
            assert!(
                crate::CSS.contains(&format!("#ctx-{id} {{ position-anchor: --ctx-{id}; }}")),
                "{id}"
            );
        }
        assert_eq!(
            html.matches(r#"type="date""#).count(),
            html.matches(r#"aria-describedby="period-date-format""#)
                .count(),
            "{html}"
        );
    }
}
