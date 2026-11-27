//! Settings › Storage and backups: what the disk allows (#632, the UI of
//! #606 D-088 and #628 D-091). Every value and every reason is the
//! engine's `capacity.get`, printed as given: the page derives nothing.

use maud::{Markup, html};
use openlens_engine::capacity::{self, Cap, Capacity};

use crate::icon;

/// One value and its reason, as a `dt` and a `dd`.
fn row(label: &str, value: &str, tag: Option<&str>, cap: &Cap) -> Markup {
    html! {
        dt { (label) }
        dd {
            strong { (value) }
            @if let Some(t) = tag { " " span.tag { (t) } }
            br;
            span.small.muted { (cap.reason) }
        }
    }
}

/// The card: the disk budget and where it comes from, the recordings
/// each project keeps a day, the fixed retention and the route-stats
/// window, each with the engine's reason. When the values do not follow
/// the disk (it cannot be read, or is not measured yet), one line says the
/// floors apply and why, never zeros (rule 11). `Err` is why the command
/// failed.
pub(crate) fn card(c: &Result<Capacity, String>) -> Markup {
    html! {
        section.card.wide #capacity aria-labelledby="capacity-h" {
            div.section-head {
                h2 #capacity-h { (icon("layers")) "What the disk allows" }
                @if let Ok(c) = c {
                    p { @if c.derived { span.tag.tone-info { "follows the disk" } } @else { span.tag { "floors" } } }
                }
            }
            @match c {
                Err(e) => p.small { "The capacity cannot be read now: " (e) "." },
                Ok(c) => {
                    @if !c.derived {
                        p.small { "The floors apply, because " (c.reason) "." }
                    }
                    dl.facts {
                        dt { "Disk budget" }
                        dd {
                            @match (c.budget_bytes, c.data_budget_bytes) {
                                (Some(b), Some(d)) => { strong { (capacity::bytes_text(b)) } " in all, " (capacity::bytes_text(d)) " of it for events and recordings" },
                                (Some(b), None) => strong { (capacity::bytes_text(b)) },
                                _ => strong { "not known yet" },
                            }
                            @if c.budget_source.starts_with(capacity::OVERRIDE_VAR) {
                                " " span.tag.tone-info { "set by environment" }
                            }
                            br;
                            span.small.muted {
                                "Source: " (c.budget_source)
                                @if let (Some(f), Some(t)) = (c.free_bytes, c.volume_bytes) {
                                    "; " (capacity::bytes_text(f)) " free on a " (capacity::bytes_text(t)) " volume"
                                }
                                "."
                            }
                        }
                        (row("Recordings per project", &format!("{} a day", c.replays_per_day.value), None, &c.replays_per_day))
                        (row("Recordings kept", &format!("{} days", c.replay_retention_days.value), Some("fixed"), &c.replay_retention_days))
                        (row("Events kept", &format!("{} days", c.event_retention_days.value), Some("fixed"), &c.event_retention_days))
                        (row("Route stats", &format!("{} windows", capacity::window::text(c.stats_window.value)), None, &c.stats_window))
                    }
                    p.small.muted {
                        "Retention stays the same on any disk: keeping personal data longer helps nobody, and you can always say how long it is kept. "
                        @match &c.measured_at {
                            Some(t) => { "Disk measured " (t.get(..16).unwrap_or(t).replace('T', " ")) " UTC, again every hour." },
                            None => "The disk is not measured yet.",
                        }
                    }
                }
            }
        }
    }
}
