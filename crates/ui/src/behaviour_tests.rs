//! The Behaviour screens (#548): Breakdown, Pages, Heatmap, Forms and
//! Feature flow.

#![allow(clippy::unwrap_used)]

use super::*;
use openlens_engine::{BreakdownRow, ClickMap, Hotspot, PageStats};

fn demo() -> ProjectInfo<'static> {
    ProjectInfo {
        id: "d",
        name: "demo",
        key: "k",
    }
}

/// The widths of the bar cells' filled parts, in page order.
fn bar_widths(html: &str) -> Vec<f64> {
    let tag = "class=\"fg\" width=\"";
    html.match_indices(tag)
        .map(|(i, _)| {
            let s = &html[i + tag.len()..];
            s[..s.find('"').unwrap()].parse().unwrap()
        })
        .collect()
}

/// The part of `html` from `from` to the next `to`.
fn part<'a>(html: &'a str, from: &str, to: &str) -> &'a str {
    let s = &html[html.find(from).unwrap()..];
    &s[..s.find(to).unwrap()]
}

fn row(value: &str, users: i64) -> BreakdownRow {
    BreakdownRow {
        value: Some(value.into()),
        users,
        sessions: users,
        events: users * 3,
        tasks: 0,
        errors: 0,
    }
}

fn page(path: &str, views: i64, hotspots: Vec<Hotspot>) -> PageStats {
    PageStats {
        path: path.into(),
        views,
        users: views / 2,
        median_scroll: 60.0,
        reach_half: 0.6,
        reach_end: 0.2,
        hotspots,
    }
}

fn hot(label: &str, clicks: i64, rage: i64) -> Hotspot {
    Hotspot {
        label: label.into(),
        clicks,
        rage,
    }
}

fn clean(html: &str) {
    assert!(!html.contains("style="), "inline style");
    assert!(!html.contains("<script"), "script");
}

#[test]
fn breakdown_tabs_mark_exactly_one_current_tab_and_keep_the_filters() {
    let f = Filters {
        environment: "staging".into(),
        role: Some("admin".into()),
        release: Some("v2".into()),
        internal: true,
        ..Filters::default()
    };
    let rows = [row("pro", 100), row("free", 25)];
    let html = breakdown(&demo(), &Board::default(), &f, "plan", &rows).into_string();
    clean(&html);
    let nav = part(&html, "aria-label=\"Dimension\"", "</nav>");
    assert_eq!(nav.matches("<a ").count(), BREAKDOWNS.len());
    assert_eq!(nav.matches("aria-current=\"page\"").count(), 1);
    let current = part(nav, "by=plan", "</a>");
    assert!(current.contains("aria-current=\"page\""), "{current}");
    for (i, _) in nav.match_indices("href=\"") {
        let href = &nav[i..i + nav[i + 6..].find('"').unwrap() + 6];
        for kept in ["env=staging", "role=admin", "release=v2", "internal=1"] {
            assert!(href.contains(kept), "{href} drops {kept}");
        }
    }
    // The bigger row has the wider bar.
    let w = bar_widths(&html);
    assert_eq!(w.len(), rows.len());
    assert!(w[0] > w[1], "{w:?}");
}

#[test]
fn pages_rows_link_to_their_heatmap_and_more_views_draw_a_wider_bar() {
    let list = [page("/pricing", 500, vec![]), page("/docs/a b", 50, vec![])];
    let html = pages(&demo(), &Board::default(), &list, &[], &Filters::default()).into_string();
    clean(&html);
    for pg in &list {
        let href = format!("href=\"{}\"", crate::manage::heatmap_href("d", &pg.path));
        assert_eq!(html.matches(&href).count(), 1, "{href}");
    }
    let w = bar_widths(&html);
    assert_eq!(w.len(), list.len());
    assert!(w[0] > w[1], "{w:?}");
}

#[test]
fn a_heatmap_without_recording_ranks_the_labels_in_click_order() {
    let list = [page(
        "/pricing",
        10,
        vec![hot("Save", 3, 0), hot("Buy", 9, 2), hot("Help", 5, 0)],
    )];
    let map = ClickMap {
        path: "/pricing".into(),
        cells: vec![(10, 4, 9, 2), (20, 8, 3, 0)],
        clicks: 12,
        viewport: 1280,
    };
    let view = |mode| HeatmapView { mode, pages: &list };
    let html = heatmap(&demo(), &map, None, &view("clicks")).into_string();
    clean(&html);
    let ranked = part(&html, "class=\"ranked-bars\"", "</ol>");
    let at = |l: &str| ranked.find(&format!(">{l}<")).unwrap();
    assert!(
        at("Buy") < at("Help") && at("Help") < at("Save"),
        "{ranked}"
    );
    // Without a recording, one line says replays add the page, and the
    // scroll depth stands in; with one, the link replaces the line.
    assert!(html.contains("class=\"muted small no-replay\""));
    assert!(html.contains("aria-label=\"Scroll depth\""));
    let rec = heatmap(
        &demo(),
        &map,
        Some("/p/d/heatmap/overlay?x"),
        &view("clicks"),
    )
    .into_string();
    assert!(!rec.contains("no-replay") && rec.contains("/p/d/heatmap/overlay?x"));
    // The modes the data has are tabs, one current; rage keeps rage only.
    let nav = part(&html, "aria-label=\"Mode\"", "</nav>");
    assert_eq!(nav.matches("<a ").count(), 3);
    assert_eq!(nav.matches("aria-current=\"page\"").count(), 1);
    let rage = heatmap(&demo(), &map, None, &view("rage")).into_string();
    clean(&rage);
    let ranked = part(&rage, "class=\"ranked-bars\"", "</ol>");
    assert!(ranked.contains(">Buy<") && !ranked.contains(">Save<"));
    assert!(rage.matches("<circle").count() < html.matches("<circle").count());
    clean(&heatmap(&demo(), &map, None, &view("scroll")).into_string());
    // A page without scroll data has no Scroll depth tab, and asking for it
    // shows the clicks.
    let mut bare = list.clone();
    bare[0].median_scroll = -1.0;
    let html = heatmap(
        &demo(),
        &map,
        None,
        &HeatmapView {
            mode: "scroll",
            pages: &bare,
        },
    )
    .into_string();
    let nav = part(&html, "aria-label=\"Mode\"", "</nav>");
    assert!(!nav.contains("mode=scroll"));
    assert!(part(nav, "<a ", "</a>").contains("aria-current=\"page\""));
}

#[test]
fn the_heatmap_is_a_behaviour_tab_current_on_its_page_and_has_an_empty_state() {
    let empty = ClickMap {
        path: String::new(),
        cells: vec![],
        clicks: 0,
        viewport: 0,
    };
    let html = heatmap(
        &demo(),
        &empty,
        None,
        &HeatmapView {
            mode: "clicks",
            pages: &[],
        },
    )
    .into_string();
    clean(&html);
    let tabs = part(&html, "aria-label=\"Behaviour\"", "</nav>");
    assert!(tabs.contains("href=\"/p/d/heatmap\" aria-current=\"page\""));
    assert!(tabs.contains("href=\"/p/d/pages\"") && !tabs.contains("/pages\" aria-current"));
    assert!(html.contains("heat-empty") && !html.contains("Most clicked"));
}

#[test]
fn forms_draw_a_longer_funnel_bar_for_a_field_more_sessions_reach() {
    use openlens_engine::forms::{FieldStats, FormStats};
    let field = |name: &str, sessions| FieldStats {
        name: name.into(),
        sessions,
        ..FieldStats::default()
    };
    let list = [FormStats {
        form: "signup".into(),
        path: "/join".into(),
        starts: 10,
        submits: 4,
        fields: vec![field("email", 9), field("phone", 3)],
        ..FormStats::default()
    }];
    let html = forms(&demo(), &Board::default(), &Filters::default(), &list).into_string();
    clean(&html);
    assert_eq!(html.matches("class=\"card table-card\"").count(), 1);
    let w = bar_widths(&html);
    assert_eq!(w.len(), 2);
    assert!(w[0] > w[1], "{w:?}");
}

#[test]
fn feature_flow_colours_nodes_by_class_and_keeps_the_table_as_a_disclosure() {
    use openlens_engine::flows::{EXIT, FlowLink, SUCCESS};
    let link = |step, from: &str, to: &str, sessions| FlowLink {
        step,
        from: from.into(),
        to: to.into(),
        sessions,
    };
    let flow = openlens_engine::Flow {
        steps: vec![
            vec![("search".into(), 10)],
            vec![(SUCCESS.into(), 6), (EXIT.into(), 4)],
        ],
        links: vec![link(0, "search", SUCCESS, 6), link(0, "search", EXIT, 4)],
        sessions: 10,
        succeeded: 6,
    };
    let html = flows(
        &demo(),
        &Board::default(),
        &Filters::default(),
        &flow,
        &[],
        None,
        false,
    )
    .into_string();
    clean(&html);
    assert!(html.contains("class=\"node s0\"") && html.contains("class=\"node ok\""));
    assert!(html.contains("<details class=\"card disclose flow-table\""));
    assert!(part(&html, "<details", "</details>").contains("As a table"));
}

#[test]
fn feature_flow_shows_the_paths_to_success_that_left_the_board() {
    // #620: the board's "Paths to value" is on Feature flow's "Paths to
    // success" tab, with the features' names.
    let flow = openlens_engine::Flow::default();
    let j = openlens_engine::Journey {
        success: "export_done".into(),
        feature: "export".into(),
        path: vec!["home".into(), "search".into()],
        users: 5,
        share: 0.5,
    };
    let b = Board {
        journeys: vec![j],
        ..Board::default()
    };
    let names = [("search".to_owned(), "Search".to_owned())];
    let render = |to_success| {
        flows(
            &demo(),
            &b,
            &Filters::default(),
            &flow,
            &names,
            None,
            to_success,
        )
        .into_string()
    };
    let html = render(true);
    clean(&html);
    let card = part(&html, r#"id="pts-h""#, "</section>");
    assert!(card.contains("Paths to success"), "{card}");
    let path = part(card, "<ul class=\"journeys\">", "</ul>");
    assert!(
        path.find(">home<") < path.find(">Search<")
            && path.find(">Search<") < path.find("export_done"),
        "{path}"
    );
    assert!(path.contains("5 users"), "{path}");
    // Not on the tab that starts from the start.
    assert!(!render(false).contains(r#"id="pts-h""#));
    // Without a path, the card says why.
    let none = flows(
        &demo(),
        &Board::default(),
        &Filters::default(),
        &flow,
        &[],
        None,
        true,
    )
    .into_string();
    assert!(part(&none, r#"id="pts-h""#, "</section>").contains("No path yet"));
}

/// A Web Vital as the engine reports it.
fn vital(name: &'static str, p75: f64, rating: &'static str) -> openlens_engine::VitalReport {
    openlens_engine::VitalReport {
        name,
        title: name,
        feels: "",
        unit: "ms",
        good: 2500.0,
        poor: 4000.0,
        p75: Some(p75),
        rating,
        samples: 121,
        worst_pages: vec![],
        dominant_cause: None,
        elements: vec![],
        devices: vec![],
        likely_cause: None,
        causes: vec![],
        actions: vec!["Make the hero image smaller"],
    }
}

#[test]
fn the_pages_page_holds_the_web_vitals_that_left_the_board() {
    // #620: the board's Web Vitals card, its "why and what to do" under
    // a vital that is not good, is on Pages (the Speed tile's page).
    let vitals = [vital("LCP", 4800.0, "poor"), vital("INP", 120.0, "good")];
    let html = pages(
        &demo(),
        &Board::default(),
        &[],
        &vitals,
        &Filters::default(),
    )
    .into_string();
    clean(&html);
    let card = part(&html, r#"id="wv-h""#, "</section>");
    assert!(card.contains("LCP") && card.contains("INP"), "{card}");
    assert_eq!(card.matches("Why and what to do").count(), 1, "{card}");
    assert!(card.contains("Make the hero image smaller"), "{card}");
    // Nothing measured: the card says so.
    let html = pages(&demo(), &Board::default(), &[], &[], &Filters::default()).into_string();
    assert!(part(&html, r#"id="wv-h""#, "</section>").contains("No measurements yet"));
}
