//! Detail pages and danger zones (#545): the entity header, mini cards,
//! KPI tiles, and the confirmation of a destructive action without script.
//!
//! A destructive button is a link to the same page with
//! `?confirm=<action>`. Given that action, the page renders as usual plus
//! an open `<dialog>` holding the confirmation form, which posts exactly
//! what the form did before (same action, method and fields): the server
//! still checks the confirmation.

use super::*;

/// A destructive action a page can ask to confirm (`?confirm=<action>`).
/// Anything else in `confirm` is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    /// Forget a person (Person page).
    Forget,
    /// Delete a flag (flag editor) or the project (Settings → Data).
    Delete,
    /// Sign out every other session (Settings → API tokens).
    SignOut,
}

impl Confirm {
    /// The action named by a `confirm` query value, if it is one we know.
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw? {
            "forget" => Some(Self::Forget),
            "delete" => Some(Self::Delete),
            "sign-out" => Some(Self::SignOut),
            _ => None,
        }
    }

    /// Its query value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forget => "forget",
            Self::Delete => "delete",
            Self::SignOut => "sign-out",
        }
    }
}

/// `page` (a path with or without a query) asking to confirm `action`.
pub(crate) fn confirm_href(page: &str, action: Confirm) -> String {
    let sep = if page.contains('?') { '&' } else { '?' };
    format!("{page}{sep}confirm={}", action.as_str())
}

/// The two letters of an entity's monogram.
pub(crate) fn monogram(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// The header of a detail page: a monogram, the title, a line of facts
/// and the page's actions.
pub(crate) fn entity(mono: &str, title: Markup, facts: Markup, actions: Markup) -> Markup {
    html! {
        div.entity {
            span.avatar.lg aria-hidden="true" { (monogram(mono)) }
            div.entity-main { h1 { (title) } (facts) }
            @if !actions.0.is_empty() { div.actions { (actions) } }
        }
    }
}

/// A KPI tile in the board's style (not a link: the page is its detail).
pub(crate) fn kpi(label: &str, value: &str, foot: &str) -> Markup {
    let id = format!(
        "kpi-{}",
        label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            })
            .collect::<String>()
    );
    html! {
        div.kpi-slot {
            div.card.kpi {
                div.kpi-head { span { (label) } }
                div.kpi-value { (value) }
                div.kpi-foot { span { (foot) } }
            }
            @if let Some(def) = crate::glossary::definition(label) {
                button.info type="button" popovertarget=(id) aria-label={ "Definition: " (label) } { (icon("info")) }
                div.kpi-pop popover="auto" id=(id) { strong { (label) } " " (def) }
            }
        }
    }
}

/// One block of a danger zone: what it does, and its button.
pub(crate) fn danger_item(title: &str, text: Markup, action: Markup) -> Markup {
    html! {
        div.item {
            div { b { (title) } p { (text) } }
            (action)
        }
    }
}

/// A danger zone: always the last card of its page.
pub(crate) fn danger_zone(id: &str, title: &str, items: Markup) -> Markup {
    html! {
        section.card.danger aria-labelledby=(id) {
            h2 id=(id) { (icon("alert")) (title) }
            (items)
        }
    }
}

/// The open confirmation dialog over the page. `form` is the existing
/// confirmation form, with a Cancel link back to the page without
/// `confirm`.
pub(crate) fn confirm_dialog(title: &str, text: Markup, form: Markup) -> Markup {
    html! {
        div.scrim {
            dialog.dialog open aria-labelledby="confirm-h" {
                h2 #confirm-h { (title) }
                p { (text) }
                (form)
            }
        }
    }
}

/// A type-to-confirm form: `word` typed again in `confirm`, posted to
/// `action`, as the forms of these actions always were.
pub(crate) fn typed_form(action: &str, word: &str, button: &str, cancel: &str) -> Markup {
    html! {
        form.confirm-form method="post" action=(action) {
            label.field { span { "Type " b.mono { (word) } " to confirm" }
                input type="text" name="confirm" required autocomplete="off" autofocus; }
            div.actions {
                a.btn href=(cancel) { "Cancel" }
                button.btn.danger-solid type="submit" { (button) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const P: ProjectInfo<'static> = ProjectInfo {
        id: "p1",
        name: "Shop",
        key: "pk_test",
    };

    fn person_page(confirm: Option<&str>) -> String {
        person(
            &P,
            "ana",
            None,
            &[],
            &[],
            &[],
            "UTC",
            true,
            Confirm::parse(confirm),
        )
        .into_string()
    }

    /// The `<form …>…</form>` blocks of `html` posting to `action`.
    fn forms_to<'a>(html: &'a str, action: &str) -> Vec<&'a str> {
        html.match_indices("<form ")
            .map(|(i, _)| &html[i..i + html[i..].find("</form>").unwrap()])
            .filter(|f| f.contains(&format!("action=\"{action}\"")))
            .collect()
    }

    /// The `name`s of the fields of a form.
    fn names(form: &str) -> Vec<&str> {
        form.split(" name=\"")
            .skip(1)
            .map(|t| &t[..t.find('"').unwrap()])
            .collect()
    }

    #[test]
    fn confirm_opens_one_dialog_with_the_forget_form_and_nothing_else_does() {
        let open = person_page(Some("forget"));
        assert_eq!(open.matches("<dialog").count(), 1, "{open}");
        let dialog = &open[open.find("<dialog").unwrap()..open.find("</dialog>").unwrap()];
        assert!(dialog.contains(" open"), "{dialog}");
        let forms = forms_to(dialog, "/p/p1/people/ana/forget");
        assert_eq!(forms.len(), 1, "{dialog}");
        assert!(forms[0].contains("method=\"post\""));
        assert_eq!(
            names(forms[0]),
            vec!["confirm"],
            "the server reads `confirm`"
        );
        assert!(forms[0].contains("autofocus"));
        // Cancel goes back to the page itself, without `confirm`.
        assert!(dialog.contains("href=\"/p/p1/people/ana\""), "{dialog}");
        for other in [None, Some("drop-everything"), Some("delete")] {
            let page = person_page(other);
            assert!(!page.contains("<dialog"), "{other:?}");
            assert!(forms_to(&page, "/p/p1/people/ana/forget").is_empty());
            // The page itself is the same; the button links to the dialog.
            assert!(page.contains("href=\"/p/p1/people/ana?confirm=forget\""));
        }
    }

    #[test]
    fn the_danger_zone_is_the_last_card_of_the_person_page() {
        for confirm in [None, Some("forget")] {
            let html = person_page(confirm);
            let last = html.rfind("class=\"card").unwrap();
            assert!(
                html[last..].starts_with("class=\"card danger\""),
                "{confirm:?}: {}",
                &html[last..]
            );
            assert!(!html.contains("style=") && !html.contains("<script"));
        }
    }

    /// A busy person's page lists their latest events only, links to all
    /// of them, and still ends on the danger zone (#590).
    #[test]
    fn the_person_page_lists_the_latest_events_and_links_to_all() {
        let rows: Vec<EventRow> = (0..crate::explore::LATEST + 40)
            .map(|i| EventRow {
                timestamp: format!("2026-11-26 {:02}:{:02}:00", 23 - i / 60, 59 - i % 60),
                event: format!("ev-{i}"),
                distinct_id: "ana".into(),
                role: None,
                path: Some("/home".into()),
                feature: None,
                release: None,
                session_id: None,
            })
            .collect();
        let html = person(&P, "ana", None, &[], &[], &rows, "UTC", true, None).into_string();
        let listed = rows
            .iter()
            .filter(|r| html.contains(&format!(">{}<", r.event)))
            .count();
        assert!(listed > 0 && listed < rows.len(), "{listed}");
        assert!(listed <= crate::explore::LATEST);
        // The newest are the ones listed.
        assert!(html.contains(">ev-0<"));
        let all = "href=\"/p/p1/events?person=ana&amp;days=90\"";
        assert!(html.matches(all).count() >= 2, "header and table head");
        // The trail ends on the person, its section one step back.
        let trail = &html[html.find("class=\"crumbs\"").unwrap()..];
        let trail = &trail[..trail.find("</nav>").unwrap()];
        assert!(trail.contains("aria-current=\"page\">ana<"), "{trail}");
        assert!(trail.contains("href=\"/p/p1/audience\""), "{trail}");
        let last = html.rfind("class=\"card").unwrap();
        assert!(html[last..].starts_with("class=\"card danger\""));
    }

    #[test]
    fn a_viewer_gets_no_danger_zone_and_no_dialog() {
        let html = person(
            &P,
            "ana",
            None,
            &[],
            &[],
            &[],
            "UTC",
            false,
            Some(Confirm::Forget),
        )
        .into_string();
        assert!(!html.contains("card danger") && !html.contains("<dialog"));
    }

    #[test]
    fn the_flag_editor_confirms_its_deletion_in_a_dialog() {
        let flag = openlens_flags::Flag {
            key: "new-editor".into(),
            description: String::new(),
            enabled: true,
            rollout: 10,
            roles: vec![],
            accounts: vec![],
            variants: vec![],
            rules: vec![],
            users: vec![],
            holdout: 0,
        };
        let plain = flag_edit(&P, Some(&flag), None, &[], None).into_string();
        let open = flag_edit(&P, Some(&flag), None, &[], Some(Confirm::Delete)).into_string();
        assert!(!plain.contains("<dialog"));
        assert!(plain.contains("href=\"/p/p1/flags-edit?key=new-editor&amp;confirm=delete\""));
        let form = forms_to(&open, "/p/p1/flags/new-editor/delete");
        assert_eq!(form.len(), 1);
        assert_eq!(names(form[0]), vec!["confirm"]);
        let last = open.rfind("class=\"card").unwrap();
        assert!(open[last..].starts_with("class=\"card danger\""));
        // Another action's name does not open it.
        let other = flag_edit(&P, Some(&flag), None, &[], Some(Confirm::Forget)).into_string();
        assert!(!other.contains("<dialog"));
    }

    #[test]
    fn the_tokens_page_confirms_sign_out_everywhere_with_the_same_post() {
        let mut v = TokensView {
            tokens: vec![],
            projects: vec![],
            created: None,
            notice: None,
            error: None,
            confirm: None,
        };
        let plain = tokens(&v).into_string();
        assert!(!plain.contains("<dialog"));
        assert!(forms_to(&plain, "/settings/sessions/revoke").is_empty());
        v.confirm = Some(Confirm::SignOut);
        let open = tokens(&v).into_string();
        let dialog = &open[open.find("<dialog").unwrap()..open.find("</dialog>").unwrap()];
        assert_eq!(forms_to(dialog, "/settings/sessions/revoke").len(), 1);
        let last = open.rfind("class=\"card").unwrap();
        assert!(open[last..].starts_with("class=\"card danger\""));
    }

    #[test]
    fn only_known_actions_parse() {
        for a in [Confirm::Forget, Confirm::Delete, Confirm::SignOut] {
            assert_eq!(Confirm::parse(Some(a.as_str())), Some(a));
        }
        for raw in ["", "Forget", "forget ", "delete-all", "<b>"] {
            assert_eq!(Confirm::parse(Some(raw)), None, "{raw}");
        }
    }
}
