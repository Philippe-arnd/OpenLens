//! The release notes card of the Releases page (#574, D-092): for the
//! newest releases recorded with a commit range, the merged pull requests
//! the git host gave, grouped Added, Fixed, Changed, with "and K more";
//! or why there are none. From `releases.list`'s notes.

use super::*;
use openlens_engine::release_git::{ListedNotes, NotesState, ReleaseNotes};

fn more(n: i64, what: &str) -> Markup {
    html! { @if n > 0 { li.muted { "and " (n) " more " (what) } } }
}

fn one(n: &ReleaseNotes) -> Markup {
    html! {
        details open[n.state == NotesState::Ready && !n.groups.is_empty()] {
            summary {
                code { (n.release) } " "
                span.muted.small {
                    (n.environment)
                    @if let Some(r) = &n.range { " · " span.mono { (r) } }
                    @if n.state == NotesState::Ready {
                        " · " (n.prs) " pull requests, " (n.commits) " commits, " (n.files) " files"
                    }
                }
                @if let Some(t) = &n.tag {
                    " "
                    @match &n.tag_url {
                        Some(u) => a.tag href=(u) rel="noopener noreferrer" { (t) },
                        None => span.tag { (t) },
                    }
                }
            }
            @if let Some(r) = &n.reason {
                p.muted.small { (r) }
            } @else if n.groups.is_empty() {
                p.muted.small { "No merged pull request in this range: its commits were pushed directly." }
            }
            @for g in &n.groups {
                h3 { (g.title) }
                ul {
                    @for i in &g.items {
                        li {
                            (i.title) " "
                            @if i.url.is_empty() {
                                span.muted { "#" (i.number) }
                            } @else {
                                a href=(i.url) rel="noopener noreferrer" { "#" (i.number) }
                            }
                        }
                    }
                }
            }
            @if n.prs_more > 0 || n.commits_more > 0 || n.files_more > 0 {
                ul.small {
                    (more(n.prs_more, "pull requests"))
                    (more(n.commits_more, "commits"))
                    (more(n.files_more, "files"))
                }
            }
        }
    }
}

/// The card: each listed release's notes, or the one reason none can be
/// shown.
pub(crate) fn notes_card(n: &ListedNotes) -> Markup {
    html! {
        section.card.wide aria-labelledby="rel-notes-h" {
            h2 #rel-notes-h { "Release notes" }
            p.muted.small { "Written from the merged pull requests of each release's commit range, read from GitHub once an hour. No author is kept." }
            @if let Some(why) = &n.unavailable {
                p.small { "Not available: " (why) }
            }
            @for r in &n.releases { (one(r)) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::release_git::{NoteGroup, NoteItem};

    fn notes(state: NotesState, reason: Option<&str>) -> ReleaseNotes {
        ReleaseNotes {
            release: "v1.4".into(),
            environment: "production".into(),
            state,
            reason: reason.map(str::to_owned),
            range: Some("aaaaaaa..bbbbbbb".into()),
            tag: None,
            tag_url: None,
            groups: Vec::new(),
            prs: 0,
            prs_more: 0,
            commits: 0,
            commits_more: 0,
            files: 0,
            files_more: 0,
            first_commit_at: None,
            fetched_at: None,
        }
    }

    #[test]
    fn notes_show_their_pull_requests_and_what_was_left_out() {
        let mut n = notes(NotesState::Ready, None);
        n.groups = vec![NoteGroup {
            title: "Fixed",
            items: vec![NoteItem {
                number: 42,
                title: "Export keeps filters".into(),
                url: "https://github.com/o/r/pull/42".into(),
                merged_at: None,
            }],
        }];
        n.prs = 130;
        n.prs_more = 30;
        let html = notes_card(&ListedNotes {
            unavailable: None,
            releases: vec![n],
        })
        .into_string();
        assert!(html.contains("https://github.com/o/r/pull/42"));
        assert!(html.contains("and 30 more pull requests"));
    }

    #[test]
    fn a_release_not_read_says_why_instead_of_an_empty_list() {
        let html = notes_card(&ListedNotes {
            unavailable: None,
            releases: vec![notes(
                NotesState::Pending,
                Some("Not read from GitHub yet: the hourly pass reads it."),
            )],
        })
        .into_string();
        assert!(html.contains("Not read from GitHub yet"));
        assert!(!html.contains("pushed directly"));
    }
}
