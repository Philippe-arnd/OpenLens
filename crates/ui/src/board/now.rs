//! "Production right now" (#614): the status strip at the top of the
//! board, one cell per question (is the app up, how is the latest release
//! judged, how are the key actions doing (#623), what incident is open,
//! what setup is left). Each cell has a
//! dot, a word (the colour is never the only cue), one line of detail and
//! a link; it shows all green when everything is fine. On a share link
//! the cells are not links and Setup is left out.

use crate::*;
use openlens_engine::board_backend::ServerLine;
use openlens_engine::board_verdict::LatestRelease;
use openlens_engine::verdict::Verdict;

/// One cell: tone (`good`, `warn`, `bad`, `muted`), what it is about,
/// the state in a word or two, one line of detail and where to act.
struct Cell {
    tone: &'static str,
    key: String,
    word: String,
    detail: String,
    href: String,
}

/// A Unix time in the reader's zone.
fn since(t: i64, tz: &str) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        local_time(&d.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)
    })
}

/// The app: up, down, traffic stopped, under maintenance, or not watched.
fn app(pid: &str, s: &openlens_engine::commands::alerts::Status, tz: &str) -> Cell {
    let at = s.since.map(|t| since(t, tz)).unwrap_or_default();
    let uptime = format!("/p/{pid}/uptime");
    let (tone, word, detail, href) = match s.state {
        "up" => ("good", "Up", "Its checks pass".to_owned(), uptime),
        "down" if !s.open.iter().any(|o| o.0 == "down") => (
            "bad",
            "Traffic stopped",
            format!("No traffic since {at}"),
            uptime,
        ),
        "down" => ("bad", "Down", format!("Down since {at}"), uptime),
        "maintenance" => (
            "warn",
            "Maintenance",
            format!("Planned work since {at}"),
            uptime,
        ),
        "off" => (
            "muted",
            "Not watched",
            "No app address to check".to_owned(),
            format!("/p/{pid}/settings#project"),
        ),
        _ => ("muted", "Waiting", "No check yet".to_owned(), uptime),
    };
    Cell {
        tone,
        key: "App".into(),
        word: word.into(),
        detail,
        href,
    }
}

/// The newest release of the environment and its verdict.
fn release(pid: &str, env: &str, l: &LatestRelease) -> Cell {
    let href = format!("/p/{pid}/releases?env={}", url_enc(env));
    let (tone, key, word, detail) = match l {
        LatestRelease::NoRelease => (
            "muted",
            "Latest release".to_owned(),
            "None yet",
            format!("No release with traffic in {env} lately"),
        ),
        LatestRelease::Unavailable { .. } => (
            "muted",
            "Latest release".to_owned(),
            "Not judged",
            "Its verdict could not be computed now".to_owned(),
        ),
        LatestRelease::Judged(r) => {
            let first = r.reasons.first().cloned().unwrap_or_default();
            let (tone, word, detail) = match r.verdict {
                Some(Verdict::Regressed) => {
                    let more = r.reasons.len().saturating_sub(1);
                    let detail = if more > 0 {
                        format!("{first} (and {more} more)")
                    } else {
                        first
                    };
                    ("bad", "Regressed", detail)
                }
                Some(Verdict::Healthy) => (
                    "good",
                    "Healthy",
                    r.previous_release
                        .as_ref()
                        .map_or(first, |p| format!("Nothing got worse than {p}")),
                ),
                Some(Verdict::Inconclusive) => ("muted", "Inconclusive", first),
                None => ("muted", "Measuring", first),
            };
            (tone, format!("Release {}", r.release), word, detail)
        }
    };
    Cell {
        tone,
        key,
        word: word.into(),
        detail,
        href,
    }
}

/// The key actions (#594 B, #623): the worst one and its success rate
/// against the length before, all fine, or none declared (a link to the
/// feature map where they are declared).
fn key_actions(pid: &str, s: &ServerLine) -> Cell {
    let routes = format!("/p/{pid}/routes");
    let seen = s.key_actions.iter().filter(|k| k.requests > 0).count();
    let (tone, word, detail, href) = match s.worst() {
        _ if s.declared == 0 => (
            "muted",
            "None declared".to_owned(),
            "Mark the routes that must never fail in the feature map".to_owned(),
            format!("/p/{pid}/features"),
        ),
        None => (
            "muted",
            "No requests".to_owned(),
            "No server request to a key action in this window".to_owned(),
            routes,
        ),
        Some(k) => {
            let now = pct(k.success_rate().unwrap_or(1.0));
            let was = k
                .previous_success_rate()
                .map(|p| format!(", was {}", pct(p)))
                .unwrap_or_default();
            if k.degraded {
                (
                    "warn",
                    format!("{} degraded", k.name),
                    format!("{now} succeed{was}"),
                    routes,
                )
            } else if seen == 1 {
                (
                    "good",
                    "All fine".to_owned(),
                    format!("{}: {now} succeed{was}", k.name),
                    routes,
                )
            } else {
                (
                    "good",
                    "All fine".to_owned(),
                    format!("{seen} key actions; the lowest, {}, {now} succeed", k.name),
                    routes,
                )
            }
        }
    };
    Cell {
        tone,
        key: "Key actions".into(),
        word,
        detail,
        href,
    }
}

/// The open incidents: how many, and of which kinds.
fn incidents(pid: &str, s: &openlens_engine::commands::alerts::Status) -> Cell {
    let n = s.open.len();
    let mut kinds: Vec<&str> = Vec::new();
    for (rule, _, _) in &s.open {
        let k = crate::live_screens::incident_kind(rule);
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    let (tone, word, detail) = if n == 0 {
        ("good", "None".to_owned(), "No open incident".to_owned())
    } else {
        let outage = s.open.iter().any(|o| o.0 == "down" || o.0 == "traffic");
        let mut detail = kinds
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        if let Some(c) = detail.get_mut(..1) {
            c.make_ascii_uppercase();
        }
        (
            if outage { "bad" } else { "warn" },
            format!("{n} open"),
            detail,
        )
    };
    Cell {
        tone,
        key: "Incidents".into(),
        word,
        detail,
        href: format!("/p/{pid}/uptime"),
    }
}

/// The setup: required steps left (admins only know them), raw roles the
/// feature map does not name.
fn setup(pid: &str, b: &Board) -> Cell {
    let gaps = b.role_gaps.len();
    let roles = |n: usize| {
        if n == 1 {
            "1 unmapped role".to_owned()
        } else {
            format!("{n} unmapped roles")
        }
    };
    let settings = format!("/p/{pid}/settings");
    let map = format!("/p/{pid}/features");
    let (tone, word, detail, href) = match b.setup {
        Some((done, total)) if done < total => (
            "warn",
            format!("{done} of {total} done"),
            if gaps > 0 {
                format!("Required steps left, and {}", roles(gaps))
            } else {
                "Required steps left in Settings".to_owned()
            },
            settings,
        ),
        _ if gaps > 0 => (
            "warn",
            roles(gaps),
            "Roles the feature map does not name yet".to_owned(),
            map,
        ),
        _ if b.map_version.is_none() => (
            "muted",
            "No map".to_owned(),
            "Roles and features come raw: no feature map yet".to_owned(),
            map,
        ),
        Some((_, total)) => (
            "good",
            "Done".to_owned(),
            format!("All {total} required steps; every role mapped"),
            settings,
        ),
        None => (
            "good",
            "Mapped".to_owned(),
            "Every role is in the feature map".to_owned(),
            map,
        ),
    };
    Cell {
        tone,
        key: "Setup".into(),
        word,
        detail,
        href,
    }
}

/// The strip; nothing when the server set none of its sources (the
/// engine's own board).
pub(super) fn strip(pid: &str, f: &Filters, b: &Board) -> Markup {
    let mut cells = Vec::new();
    if let Some(s) = &b.alerts {
        cells.push(app(pid, s, &f.tz));
    }
    if let Some(l) = &b.latest_release {
        cells.push(release(pid, &f.environment, l));
    }
    if let Some(s) = b
        .server
        .as_ref()
        .filter(|_| b.alerts.is_some() || b.latest_release.is_some())
    {
        cells.push(key_actions(pid, s));
    }
    if let Some(s) = &b.alerts {
        cells.push(incidents(pid, s));
    }
    if !shared() && (b.alerts.is_some() || b.latest_release.is_some()) {
        cells.push(setup(pid, b));
    }
    if cells.is_empty() {
        return html! {};
    }
    html! {
        section.card.now aria-label="Production right now" {
            @for c in &cells {
                @let body = html! {
                    span.k { span.state-dot aria-hidden="true" {} (c.key) }
                    strong.v { (c.word) }
                    span.s { (c.detail) }
                };
                @if shared() {
                    div class={ "tone-" (c.tone) } { (body) }
                } @else {
                    a class={ "tone-" (c.tone) } href=(c.href) { (body) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{between, full, judged, page};
    use super::*;
    use openlens_engine::board_backend::KeyAction;

    fn sign_in(errors: i64, before: i64, degraded: bool) -> KeyAction {
        KeyAction {
            feature: "sign-in".into(),
            name: "Sign-in".into(),
            requests: 1_000,
            errors,
            previous_requests: 1_000,
            previous_errors: before,
            degraded,
        }
    }

    /// The Key actions cell of a board with this server side.
    fn cell(s: ServerLine) -> String {
        let b = Board {
            latest_release: Some(judged(8)),
            server: Some(s),
            ..full()
        };
        let html = page(&b);
        let strip = between(&html, r#"aria-label="Production right now""#, "</section>");
        strip
            .split(r#"class="tone-"#)
            .find(|c| c.contains("Key actions"))
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn the_key_actions_cell_names_the_degraded_one_says_all_fine_or_links_to_the_map() {
        // Degraded: its name, its success rate now and before, in words.
        let c = cell(ServerLine {
            declared: 1,
            key_actions: vec![sign_in(84, 1, true)],
            ..ServerLine::default()
        });
        assert!(!c.starts_with("good"), "{c}");
        assert!(c.contains("Sign-in degraded"), "{c}");
        let (now, was) = (pct(1.0 - 84.0 / 1_000.0), pct(1.0 - 1.0 / 1_000.0));
        assert!(c.contains(&format!("{now} succeed, was {was}")), "{c}");
        assert!(c.contains("/p/p1/routes"), "{c}");
        // Fine: green, and the key action still named.
        let c = cell(ServerLine {
            declared: 1,
            key_actions: vec![sign_in(1, 1, false)],
            ..ServerLine::default()
        });
        assert!(c.starts_with("good") && c.contains("All fine"), "{c}");
        // None declared: says so and leads to the feature map.
        let c = cell(ServerLine::default());
        assert!(
            c.contains("None declared") && c.contains("/p/p1/features"),
            "{c}"
        );
        assert!(!c.contains('%'), "no empty number: {c}");
        // Declared, but no request in the window: no number either.
        let c = cell(ServerLine {
            declared: 1,
            ..ServerLine::default()
        });
        assert!(c.contains("No requests") && !c.contains('%'), "{c}");
    }

    #[test]
    fn a_board_that_did_not_read_the_server_side_has_no_key_actions_cell() {
        let b = Board {
            latest_release: Some(judged(8)),
            ..full()
        };
        let html = page(&b);
        let strip = between(&html, r#"aria-label="Production right now""#, "</section>");
        assert!(!strip.contains("Key actions"), "{strip}");
    }
}
