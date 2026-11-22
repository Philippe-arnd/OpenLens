//! Line icons drawn for OpenLens (24×24 grid, 1.5 px strokes, currentColor).
//!
//! Original artwork made of simple geometric primitives (clean room: no
//! icon set was copied). Rendered once per page as an inline SVG sprite and
//! referenced with `<use>`, so pages stay small and need no request.

use maud::{Markup, PreEscaped, html};

const ICONS: &[(&str, &str)] = &[
    (
        "board",
        r#"<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="4" rx="1.5"/><rect x="13" y="10" width="7" height="10" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/>"#,
    ),
    (
        "events",
        r#"<path d="M9 6h11M9 12h11M9 18h11"/><circle cx="5" cy="6" r=".6"/><circle cx="5" cy="12" r=".6"/><circle cx="5" cy="18" r=".6"/>"#,
    ),
    (
        "play",
        r#"<circle cx="12" cy="12" r="8.5"/><path d="M10 8.8v6.4l5.2-3.2z"/>"#,
    ),
    ("flag", r#"<path d="M5 21V4M5 4h11l-2.5 4L16 12H5"/>"#),
    (
        "users",
        r#"<circle cx="9" cy="8" r="3"/><path d="M3.5 19c.4-3 2.7-5 5.5-5s5.1 2 5.5 5"/><circle cx="17" cy="9" r="2.3"/><path d="M16.5 14.2c2.4.2 4 1.9 4.3 4.3"/>"#,
    ),
    ("pulse", r#"<path d="M3 12h4l2.2-6 4.6 12 2.2-6h5"/>"#),
    (
        "clock",
        r#"<circle cx="12" cy="12" r="8"/><path d="M12 8v4l2.8 1.8"/>"#,
    ),
    (
        "task",
        r#"<rect x="4" y="4" width="16" height="16" rx="3.5"/><path d="M8.5 12.2l2.4 2.4 4.6-5.2"/>"#,
    ),
    (
        "alert",
        r#"<path d="M10.3 4.8a2 2 0 013.4 0l7 12.2a2 2 0 01-1.7 3H5a2 2 0 01-1.7-3z"/><path d="M12 10v3.5M12 16.6v.1"/>"#,
    ),
    (
        "bolt",
        r#"<path d="M13 3L5.5 13.5H11l-1 7.5 7.5-10.5H12z"/>"#,
    ),
    (
        "layers",
        r#"<path d="M12 4l8 4-8 4-8-4z"/><path d="M4 12l8 4 8-4M4 16l8 4 8-4"/>"#,
    ),
    (
        "shield",
        r#"<path d="M12 3.5l7 2.7v5.6c0 4.2-2.9 7.3-7 8.7-4.1-1.4-7-4.5-7-8.7V6.2z"/>"#,
    ),
    (
        "release",
        r#"<circle cx="12" cy="12" r="3"/><path d="M3 12h6M15 12h6"/>"#,
    ),
    (
        "route",
        r#"<circle cx="6" cy="18" r="2"/><circle cx="18" cy="6" r="2"/><path d="M8 18h5.5a3 3 0 000-6h-3a3 3 0 010-6H16"/>"#,
    ),
    ("funnel", r#"<path d="M4 5h16l-6 7.5V18l-4 2v-7.5z"/>"#),
    (
        "star",
        r#"<path d="M12 4.2l2.3 4.7 5.2.8-3.8 3.6.9 5.2L12 16l-4.6 2.5.9-5.2-3.8-3.6 5.2-.8z"/>"#,
    ),
    (
        "down",
        r#"<path d="M3 7.5l6 6 4-4 8 8"/><path d="M21 12v5.5h-5.5"/>"#,
    ),
    (
        "up",
        r#"<path d="M3 16.5l6-6 4 4 8-8"/><path d="M21 12V6.5h-5.5"/>"#,
    ),
    (
        "sun",
        r#"<circle cx="12" cy="12" r="3.6"/><path d="M12 3v1.8M12 19.2V21M3 12h1.8M19.2 12H21M5.6 5.6l1.3 1.3M17.1 17.1l1.3 1.3M5.6 18.4l1.3-1.3M17.1 6.9l1.3-1.3"/>"#,
    ),
    (
        "moon",
        r#"<path d="M19.5 14.5A7.5 7.5 0 119.5 4.5a6 6 0 0010 10z"/>"#,
    ),
    (
        "auto",
        r#"<rect x="3.5" y="4.5" width="17" height="11.5" rx="2"/><path d="M9 20h6M12 16v4"/>"#,
    ),
    (
        "logout",
        r#"<path d="M14 4.5h4.5v15H14"/><path d="M10 8l-4 4 4 4M6 12h10"/>"#,
    ),
    (
        "idea",
        r#"<path d="M9.5 18h5M10.5 21h3"/><path d="M12 3.5a5.8 5.8 0 00-3.7 10.3c.8.7 1.2 1.4 1.2 2.4h5c0-1 .4-1.7 1.2-2.4A5.8 5.8 0 0012 3.5z"/>"#,
    ),
    (
        "spark",
        r#"<path d="M12 3.5v4M12 16.5v4M3.5 12h4M16.5 12h4M6 6l2.6 2.6M15.4 15.4L18 18M6 18l2.6-2.6M15.4 8.6L18 6"/>"#,
    ),
    (
        "ban",
        r#"<circle cx="12" cy="12" r="8"/><path d="M6.4 6.4l11.2 11.2"/>"#,
    ),
    (
        "gap",
        r#"<circle cx="7" cy="12" r="3"/><circle cx="17" cy="12" r="3"/><path d="M10 12h1M13 12h1"/>"#,
    ),
];

/// The sprite, once per page.
pub fn sprite() -> Markup {
    let mut s = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" style="display:none" aria-hidden="true">"#,
    );
    for (name, body) in ICONS {
        s.push_str(&format!(
            r#"<symbol id="i-{name}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{body}</symbol>"#
        ));
    }
    s.push_str("</svg>");
    PreEscaped(s)
}

/// One icon (decorative: the text next to it carries the meaning).
pub fn icon(name: &str) -> Markup {
    html! {
        svg.icon aria-hidden="true" focusable="false" { use href={ "#i-" (name) } {} }
    }
}
