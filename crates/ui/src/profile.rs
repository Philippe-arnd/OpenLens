//! My profile: name and time zone, password, appearance (#215, #428, #438).
//!
//! Moved out of `team.rs` as is (#546).

use super::*;

/// The password card of *My profile* (#428).
pub struct MePassword {
    /// Password sign-in is on.
    pub enabled: bool,
    /// Signed in with the admin token (no account yet).
    pub owner: bool,
    /// This account has a password.
    pub has_password: bool,
    /// The notice is an error.
    pub error: bool,
    /// The second factor (#440).
    pub totp: crate::TotpCard,
}

/// The tabs of *My profile* (#438).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeTab {
    /// Name and time zone.
    #[default]
    Profile,
    /// Password.
    SignIn,
    /// Theme and sidebar.
    Appearance,
}

impl MeTab {
    fn title(self) -> &'static str {
        match self {
            Self::Profile => "My profile",
            Self::SignIn => "Sign-in",
            Self::Appearance => "Appearance",
        }
    }
}

/// How the reader wants pages to look (#438).
#[derive(Debug, Clone, Copy, Default)]
pub struct MeLook<'a> {
    /// `light`, `dark`, or `None` to follow the system.
    pub theme: Option<&'a str>,
    /// The sidebar starts folded.
    pub nav_folded: bool,
    /// Saved for the account (across browsers), not only this browser.
    pub per_account: bool,
}

/// My profile: the time zone I read times in, my name (#215, #216), my
/// password (#428) and how pages look (#438), one tab each.
pub fn me(
    tab: MeTab,
    name: Option<&str>,
    email: Option<&str>,
    tz: Option<&str>,
    notice: Option<&str>,
    pw: &MePassword,
    look: &MeLook<'_>,
) -> Markup {
    layout(
        tab.title(),
        None,
        html! {
            div.heading { div {
                h1 { "My profile" }
                p.muted { @if let Some(e) = email { (e) } @else { "Owner (admin token)" } }
            } }
            @if let Some(n) = notice {
                @if pw.error { p.alert role="alert" { (n) } } @else { p.notice role="status" { (n) } }
            }
            @if tab == MeTab::SignIn && !pw.enabled {
                section.card { p.muted { "Password sign-in is turned off on this server: you sign in with a link or your identity provider." } }
            }
            @if tab == MeTab::SignIn && pw.enabled {
                section.card.wide #password {
                    @if pw.owner {
                        h2.violet { (icon("shield")) "Create your admin account" }
                        p.small { "You signed in with the admin token. Create an account with your email and a password to sign in without it. The token keeps working as the way back in (a lost password, a locked account)." }
                        form.settings method="post" action="/me/password" {
                            label { "Email " input type="email" name="email" autocomplete="username" required maxlength="200"; }
                            label { "Password " input type="password" name="password" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            label { "Again " input type="password" name="again" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            button type="submit" { "Create my account" }
                        }
                    } @else {
                        h2.violet { (icon("shield")) (if pw.has_password { "Change password" } else { "Set a password" }) }
                        form.settings method="post" action="/me/password" {
                            input type="email" name="email" value=[email] autocomplete="username" hidden;
                            @if pw.has_password {
                                label { "Current password " input type="password" name="current" autocomplete="current-password" required maxlength="200"; }
                            }
                            label { "New password " input type="password" name="password" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            label { "Again " input type="password" name="again" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            button type="submit" { "Save the password" }
                        }
                        p.muted.small { "At least 12 characters; a few words work well. Saving it signs out your other sessions." }
                    }
                }
                (crate::totp::totp_card(&pw.totp))
            }
            @if tab == MeTab::Appearance { (appearance(look)) }
            @if tab == MeTab::Profile {
                form.card.wide.form-card method="post" action="/me" aria-labelledby="me-h" {
                    div.section-head { h2 #me-h { (icon("users")) "Profile" } }
                    div.form-grid {
                        @if email.is_some() {
                            label.field { "Name" input type="text" name="name" value=[name] maxlength="80"; }
                        }
                        label.field { "Time zone"
                            input type="text" name="timezone" value=[tz] placeholder="the project's" maxlength="64" list="tz-all";
                            small { "Every time and every day on the boards follows this zone (an IANA name like Europe/Paris). Empty: each project's own zone." }
                        }
                    }
                    (zone_list())
                    (crate::project_settings::form_foot("Save", html! { "Only you see the pages this way." }))
                }
            }
        },
    )
}

/// The theme choice of a look: `light`, `dark`, or `auto` (the system),
/// whatever was stored, so exactly one option is ever checked.
fn theme_choice(look: &MeLook<'_>) -> &'static str {
    match look.theme {
        Some("light") => "light",
        Some("dark") => "dark",
        _ => "auto",
    }
}

/// *Appearance* (#438, #546): the theme as a segmented control with a
/// swatch per option, and the folded menu as a switch. The field names
/// and values are the ones `/me/appearance` reads.
fn appearance(look: &MeLook<'_>) -> Markup {
    let current = theme_choice(look);
    html! {
        form.card.wide.form-card method="post" action="/me/appearance" aria-labelledby="look-h" {
            div.section-head { h2 #look-h { (icon("sun")) "Appearance" } }
            fieldset {
                legend { "Theme" }
                div.seg.themes {
                    @for (value, label) in [("light", "Light"), ("dark", "Dark"), ("auto", "Follow the system")] {
                        label {
                            input type="radio" name="theme" value=(value) checked[current == value];
                            span class={ "swatch " (value) } aria-hidden="true" {}
                            (label)
                        }
                    }
                }
            }
            label.switch {
                input type="checkbox" role="switch" name="nav_folded" value="on" checked[look.nav_folded];
                span { b { "Start with the menu folded" } small { "Icons only." } }
            }
            (crate::project_settings::form_foot("Save", html! {
                @if look.per_account { "Saved with your account: it follows you to every browser you sign in from." }
                @else { "Saved in this browser (the admin token has no account)." }
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_settings::tests::inputs;

    #[test]
    fn the_theme_control_has_exactly_one_checked_option() {
        // Whatever was stored, one option is checked: the stored theme, or
        // the system's for a value the form does not offer.
        for (stored, expect) in [
            (None, "auto"),
            (Some("light"), "light"),
            (Some("dark"), "dark"),
            (Some("sepia"), "auto"),
        ] {
            for nav_folded in [false, true] {
                let look = MeLook {
                    theme: stored,
                    nav_folded,
                    per_account: true,
                };
                let html = appearance(&look).into_string();
                assert!(
                    html.contains(r#"action="/me/appearance""#)
                        && html.contains(r#"method="post""#)
                );
                let radios: Vec<&str> = inputs(&html)
                    .into_iter()
                    .filter(|t| t.contains(r#"type="radio""#))
                    .collect();
                assert!(radios.iter().all(|t| t.contains(r#"name="theme""#)));
                let checked: Vec<&&str> =
                    radios.iter().filter(|t| t.contains(" checked")).collect();
                assert_eq!(checked.len(), 1, "{stored:?}");
                assert!(
                    checked[0].contains(&format!(r#"value="{expect}""#)),
                    "{stored:?}"
                );
                // The folded menu: the same field, as a switch.
                let nav = inputs(&html)
                    .into_iter()
                    .find(|t| t.contains(r#"name="nav_folded""#))
                    .unwrap_or_default();
                assert!(nav.contains(r#"role="switch""#) && nav.contains(r#"value="on""#));
                assert_eq!(nav.contains(" checked"), nav_folded);
                assert!(!html.contains(" style=") && !html.contains("<script"));
            }
        }
    }
}
