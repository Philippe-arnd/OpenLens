//! Web dashboard: server-rendered HTML (maud) and SVG charts, no client-side
//! JavaScript (D-011).
//!
//! Minimal, colourful and efficient, with a playful touch: stat tiles that
//! tilt a little on hover, a colour-blob logo that slowly changes shape, friendly copy. Motion
//! respects `prefers-reduced-motion`; dark mode follows the system.
//!
//! Layer: L4 (see `docs/architecture.md`).

use maud::{Markup, PreEscaped, html};

mod announcements;
mod audience;
mod board;
mod board_job;
mod capacity;
mod check_ins;
mod components;
mod cost;
mod delivery;
mod dependencies_table;
pub use dependencies_table::ErrorsBackend;
mod detail;
mod entry;
mod environments;
mod events;
mod explore;
mod feature_page;
mod features;
mod glossary;
mod goals;
mod goto;
mod icons;
mod live;
mod live_screens;
mod maintenance;
mod next_actions;
mod manage;
mod people;
mod profile;
mod project_settings;
mod release_notes;
mod release_runtime;
mod release_verdict;
mod releases;
mod routes_page;
mod settings;
mod shell;
mod tables;
pub(crate) use tables::family_label;
mod slo;
mod status_page;
mod team;
mod totp;
mod vitals;
mod world;
pub use announcements::{
    NoticeForm, NoticeRow, NoticesView, OfferRow, linked, notice_page, notices,
};
pub use audience::*;
pub use board::*;
pub use board_job::{JobView, board_job};
pub use check_ins::{CheckInRow, CheckInsView, check_ins_card};
pub use components::*;
pub use cost::board_confirm;
pub use detail::Confirm;
pub use entry::{login, login_continue, message, message_html, welcome};
pub use events::events;
pub use explore::*;
pub use feature_page::*;
pub use features::*;
pub(crate) use glossary::{th_def, th_num, vital_info};
pub use goals::*;
pub use goto::{GotoPlace, goto_results, goto_sections};
pub use icons::icon;
pub use live::live;
pub use maintenance::{MaintenanceRow, MaintenanceView, maintenance_card};
pub use next_actions::next_actions;
pub use manage::*;
use openlens_engine::{Board, EventRow, Filters};
pub use people::*;
pub use profile::*;
pub use project_settings::*;
pub use releases::*;
pub use routes_page::routes_page;
pub use settings::*;
pub(crate) use shell::{Crumbs, NAV, layout, layout_in, layout_with};
pub use shell::{NavProject, set_admin_source, set_icon_source, set_nav_project_source};
pub use slo::{AvailabilityView, availability_card};
pub use status_page::{
    StatusComponentRow, StatusPageCard, state_words, status_page, status_page_card,
};
pub use team::*;
pub use totp::{TotpCard, login_code, totp_recovery_codes};

/// Stylesheet, served at [`css_href`] (and at `/assets/app.css` for pages
/// cached before #579).
pub const CSS: &str = include_str!("../assets/app.css");

/// The stylesheet's file name under `/assets/app/`: the start of its
/// SHA-256 and `.css`. A new release changes the name, so the browser may
/// keep the file for a year (#579).
pub fn css_file() -> &'static str {
    static FILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        use sha2::Digest;
        use std::fmt::Write;
        let digest = sha2::Sha256::digest(CSS.as_bytes());
        let mut name = String::with_capacity(20);
        for b in digest.iter().take(8) {
            let _ = write!(name, "{b:02x}");
        }
        name.push_str(".css");
        name
    })
}

/// Where every page links the stylesheet: `/assets/app/<hash>.css`.
pub fn css_href() -> &'static str {
    static HREF: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HREF.get_or_init(|| format!("/assets/app/{}", css_file()))
}

/// The favicon: the logo blob, still.
pub const FAVICON: &str = include_str!("../assets/favicon.svg");

/// Project summary used in the header and empty states.
pub struct ProjectInfo<'a> {
    /// Id.
    pub id: &'a str,
    /// Display name.
    pub name: &'a str,
    /// Capture key.
    pub key: &'a str,
}

/// A newer release (#198), and whether it fixes security advisories (#416).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// Its tag, e.g. `v0.5.0`.
    pub tag: String,
    /// Its notes carry a *Security* section.
    pub security: bool,
}

static UPDATE: std::sync::RwLock<Option<Update>> = std::sync::RwLock::new(None);

/// Record the newest release when it is newer than this one (the server
/// checks once a day), so every page's footer can say so.
pub fn set_update_available(update: Option<Update>) {
    if let Ok(mut u) = UPDATE.write() {
        *u = update;
    }
}

fn update_available() -> Option<Update> {
    UPDATE.read().ok().and_then(|u| u.clone())
}

/// The footer's update notice: louder for a security release.
fn update_notice(u: &Update) -> Markup {
    let href = format!(
        "https://github.com/Philippe-arnd/OpenLens/releases/tag/{}",
        u.tag
    );
    html! {
        @if u.security {
            " · " a.update-security href=(href) { "Security update available: " (u.tag) " — fixes published advisories, upgrade soon" }
        } @else {
            " · " a href=(href) { "Update available: " (u.tag) }
        }
    }
}

/// Format a count compactly: 1,284 / 12.9K / 4.2M.
pub fn compact(n: i64) -> String {
    let a = n.unsigned_abs() as f64;
    let sign = if n < 0 { "-" } else { "" };
    if a < 10_000.0 {
        let s = n.unsigned_abs().to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        format!("{sign}{out}")
    } else if a < 1_000_000.0 {
        format!("{sign}{:.1}K", a / 1_000.0)
    } else {
        format!("{sign}{:.1}M", a / 1_000_000.0)
    }
}

/// Percent-encode a query parameter value.
pub fn url_enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Format a ratio as a percentage.
pub fn pct(r: f64) -> String {
    if r > 0.0 && r < 0.001 {
        "<0.1%".into()
    } else {
        format!("{:.1}%", r * 100.0)
    }
}

/// A UTC timestamp (`2026-11-23 10:04:05.123`, `T` or space, optional
/// `Z`) shown in the reader's time zone as `YYYY-MM-DD HH:MM[:SS]` (#215).
/// Unparseable input is shown as it came.
pub fn local_time(ts: &str, tz: &str, seconds: bool) -> String {
    let fmt = if seconds {
        "%Y-%m-%d %H:%M:%S"
    } else {
        "%Y-%m-%d %H:%M"
    };
    let t = ts.trim().trim_end_matches('Z').replace('T', " ");
    let t = t.split('+').next().unwrap_or(&t).to_owned();
    let parsed = chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M"));
    match (parsed, tz.parse::<chrono_tz::Tz>()) {
        (Ok(n), Ok(z)) => n.and_utc().with_timezone(&z).format(fmt).to_string(),
        (Ok(n), Err(_)) => n.format(fmt).to_string(),
        _ => ts
            .get(..if seconds { 19 } else { 16 })
            .unwrap_or(ts)
            .to_owned(),
    }
}

/// Short label of a time zone for headers ("Europe/Paris" → "Paris").
fn tz_label(tz: &str) -> &str {
    tz.rsplit('/').next().unwrap_or(tz)
}

/// Format seconds as "4m 12s".
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

thread_local! {
    static SHARED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The reader's display choices (#300): the theme and the sidebar. The
/// server says where to read them ([`set_prefs_source`]); `layout()`
/// writes them on `<html>`, so nothing rewrites the page afterwards.
#[derive(Debug, Clone, Copy, Default)]
pub struct Prefs {
    /// `light` or `dark`; `None` follows the system.
    pub theme: Option<&'static str>,
    /// The sidebar is folded.
    pub nav_collapsed: bool,
}

static PREFS: std::sync::OnceLock<fn() -> Prefs> = std::sync::OnceLock::new();

/// Where pages read the current request's [`Prefs`] (the server keeps them
/// per request). Set once; later calls are ignored.
pub fn set_prefs_source(source: fn() -> Prefs) {
    let _ = PREFS.set(source);
}

fn prefs() -> Prefs {
    PREFS.get().map_or_else(Prefs::default, |f| f())
}

/// Is the page being rendered for a share link (#229)?
fn shared() -> bool {
    SHARED.with(std::cell::Cell::get)
}

/// Render a page for a read-only share link (#229): no sidebar, no
/// filters, no links that need a login; a banner says what it is.
pub fn render_shared(f: impl FnOnce() -> Markup) -> Markup {
    SHARED.with(|s| s.set(true));
    let m = f();
    SHARED.with(|s| s.set(false));
    m
}

/// Raw HTML escape hatch for tests.
pub fn render(m: Markup) -> String {
    let PreEscaped(s) = m;
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// Whether `html` holds an ISO time (`…6T20:49`) rather than the
    /// `YYYY-MM-DD HH:MM` the screens print (#556).
    pub(crate) fn has_iso_time(html: &str) -> bool {
        html.as_bytes().windows(5).any(|w| {
            w[0].is_ascii_digit()
                && w[1] == b'T'
                && w[2].is_ascii_digit()
                && w[3].is_ascii_digit()
                && w[4] == b':'
        })
    }

    #[test]
    fn a_security_release_gets_a_louder_notice() {
        let plain = update_notice(&Update {
            tag: "v0.5.0".into(),
            security: false,
        })
        .into_string();
        let sec = update_notice(&Update {
            tag: "v0.5.0".into(),
            security: true,
        })
        .into_string();
        assert!(plain.contains("Update available: v0.5.0"));
        assert!(!plain.contains("update-security"));
        assert!(sec.contains("class=\"update-security\""));
        assert!(sec.contains("Security update available: v0.5.0"));
        assert!(sec.contains("releases/tag/v0.5.0"));
    }

    /// Every card and tile of the board opens its detail (#409): a new one
    /// must use `tile_to` / `to(...)`, or say `no_detail` on purpose.
    #[test]
    fn every_board_card_and_tile_leads_somewhere() {
        let src = include_str!("board/mod.rs");
        let body = [
            &src[src.find("pub fn board(").unwrap()..],
            include_str!("board/focus.rs"),
            include_str!("board/lead.rs"),
            include_str!("board/now.rs"),
            include_str!("board/tiles.rs"),
            include_str!("board/charts.rs"),
            include_str!("board/features.rs"),
            include_str!("board/in_flight.rs"),
        ]
        .concat();
        assert!(
            !body.contains("(tile(\""),
            "a board tile without a detail: use kpi_to"
        );
        // Every card title (`h2.x`, `h2 id=…`, `h2 {`) opens its page, or
        // says why not.
        for line in body.lines().filter(|l| {
            let l = l.trim_start();
            l.starts_with("h2.") || l.starts_with("h2 ") || l.starts_with("h2#")
        }) {
            assert!(
                line.contains("(to(") || line.contains("no_detail"),
                "card without a detail: {line}"
            );
        }
        // The other cards left for the pages that own them (#620): no
        // disclosure stays on the board, and the one block no card or tile
        // pointed at, the paths to a first success, has its link.
        assert!(!body.contains("details.card"), "a disclosure on the board");
        assert!(body.contains(r#"a href=(detail_url(pid, "/flows?to=success", f))"#));
        let f = Filters {
            environment: "production".into(),
            role: Some("admin".into()),
            internal: true,
            ..Filters::default()
        };
        assert_eq!(
            detail_url("d", "/errors", &f),
            "/p/d/errors?env=production&role=admin&internal=1"
        );
        assert_eq!(
            detail_url("d", "/breakdown?by=role", &f),
            "/p/d/breakdown?by=role&env=production&role=admin&internal=1"
        );
        let shared = render_shared(|| to("/x", "Releases")).into_string();
        assert_eq!(shared, "Releases", "no link on a share link");
    }

    #[test]
    fn plain_wording_replaces_the_card_text_and_keeps_the_original() {
        let mut b = Board {
            total_events: 10,
            ..Board::default()
        };
        b.insights.push(openlens_engine::Insight {
            kind: "churn_point",
            title: "72% of churned users were last seen in billing".into(),
            evidence: "113 of 156 churned users".into(),
            next_step: "Look at what happens in billing.".into(),
            score: 1.0,
            subject: "billing".into(),
            figure: None,
        });
        let p = ProjectInfo {
            id: "p",
            name: "P",
            key: "pk",
        };
        let plain = [(
            "Most people who left (72%) were in billing last".to_owned(),
            "Check the billing page first.".to_owned(),
        )];
        let html = render(board(
            &p,
            &b,
            &Filters::default(),
            "http://x",
            &plain,
            None,
            &[],
        ));
        assert!(html.contains("Most people who left (72%)"));
        assert!(html.contains("Original: 72% of churned users were last seen in billing"));
        assert!(html.contains("Check the billing page first."));
        let html = render(board(
            &p,
            &b,
            &Filters::default(),
            "http://x",
            &[],
            None,
            &[],
        ));
        assert!(
            html.contains("72% of churned users were last seen in billing")
                && !html.contains("Original:")
        );
    }
}

#[cfg(test)]
mod behaviour_tests;

#[cfg(test)]
mod diff_tests {
    #[test]
    fn line_diff_keeps_common_lines() {
        let d = super::line_diff("a\nb\nc\n", "a\nx\nc\nd\n");
        let s: String = d.iter().map(|(c, l)| format!("{c}{l};")).collect();
        assert_eq!(s, " a;-b;+x; c;+d;");
        assert!(super::line_diff("", "").is_empty());
    }
}

#[cfg(test)]
mod motion_tests;

#[cfg(test)]
mod css_budget_tests {
    /// The gzip budget of `app.css` (docs/design/ui-v2/README.md §3.1),
    /// revised in #579 to the pruned size rounded up to the next kB: the
    /// hashed, immutable URL makes it a once-per-release download.
    const BUDGET_GZ: usize = 23 * 1024;

    fn gzip_best(s: &str) -> usize {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(s.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        gz.finish().unwrap_or_else(|e| panic!("{e}")).len()
    }

    #[test]
    fn the_stylesheet_stays_within_its_gzip_budget() {
        let size = gzip_best(crate::CSS);
        assert!(
            size <= BUDGET_GZ,
            "app.css is {size} bytes gzipped, over its {BUDGET_GZ}-byte budget (§3.1)"
        );
    }
}
