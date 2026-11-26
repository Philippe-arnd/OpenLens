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
            @if tab == MeTab::Appearance {
                section.card.wide {
                    h2.violet { (icon("sun")) "Appearance" }
                    form.settings method="post" action="/me/appearance" {
                        fieldset {
                            legend { "Theme" }
                            label.check { input type="radio" name="theme" value="auto" checked[look.theme.is_none()]; " Follow the system" }
                            label.check { input type="radio" name="theme" value="light" checked[look.theme == Some("light")]; " Light" }
                            label.check { input type="radio" name="theme" value="dark" checked[look.theme == Some("dark")]; " Dark" }
                        }
                        label.check { input type="checkbox" name="nav_folded" value="on" checked[look.nav_folded]; " Start with the menu folded (icons only)" }
                        button type="submit" { "Save" }
                    }
                    p.muted.small {
                        @if look.per_account { "Saved with your account: it follows you to every browser you sign in from." }
                        @else { "Saved in this browser (the admin token has no account)." }
                    }
                }
            }
            @if tab == MeTab::Profile {
            section.card.wide {
                form.settings method="post" action="/me" {
                    @if email.is_some() {
                        label { "Name " input type="text" name="name" value=[name] maxlength="80"; }
                    }
                    label { "Time zone " input type="text" name="timezone" value=[tz] placeholder="the project's" maxlength="64" list="tz-all"; }
                    (zone_list())
                    button type="submit" { "Save" }
                }
                p.muted.small { "Every time and every day on the boards follows this zone (an IANA name like Europe/Paris). Empty: each project's own zone." }
            }
            }
        },
    )
}
