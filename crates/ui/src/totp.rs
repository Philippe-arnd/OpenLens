//! The second factor for password sign-in (#440): the code step of the
//! login, the card on *My profile → Sign-in*, and the recovery codes page.

use super::*;

/// Where the account's second factor stands, for the *Sign-in* tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TotpCard {
    /// Not offered (owner session, no password, password sign-in off).
    #[default]
    Hidden,
    /// Off: offer to set it up.
    Off,
    /// Set up started: scan the code, then confirm.
    Pending {
        /// The QR code, inline SVG made by the server (trusted markup).
        qr_svg: Option<String>,
        /// The key to type by hand, in groups of four.
        key: String,
    },
    /// On.
    On {
        /// Since when (UTC, readable).
        since: String,
        /// Recovery codes left unused.
        recovery_left: usize,
    },
}

/// The second-factor card of *My profile → Sign-in*.
pub(crate) fn totp_card(card: &TotpCard) -> Markup {
    html! {
        @match card {
            TotpCard::Hidden => {}
            TotpCard::Off => {
                section.card.wide #totp {
                    h2.violet { (icon("shield")) "Authenticator app" }
                    p.small { "Ask for a 6-digit code from an authenticator app after your password. Login links, single sign-on and the admin token do not ask for it." }
                    form.settings method="post" action="/me/totp/start" {
                        button type="submit" { "Set up an authenticator app" }
                    }
                }
            }
            TotpCard::Pending { qr_svg, key } => {
                section.card.wide #totp {
                    h2.violet { (icon("shield")) "Set up an authenticator app" }
                    p.small { "Scan this code with your authenticator app, or type the key by hand. Then enter the code it shows, with your password." }
                    @if let Some(svg) = qr_svg {
                        div.qr aria-label="QR code of the key" { (PreEscaped(svg)) }
                    }
                    p { "Key: " code.mono { (key) } }
                    form.settings method="post" action="/me/totp/enable" {
                        label { "Code from the app " input type="text" name="code" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9]{6}" required maxlength="6"; }
                        label { "Current password " input type="password" name="current" autocomplete="current-password" required maxlength="200"; }
                        button type="submit" { "Turn it on" }
                    }
                    form method="post" action="/me/totp/cancel" {
                        button.ghost type="submit" { "Cancel" }
                    }
                }
            }
            TotpCard::On { since, recovery_left } => {
                section.card.wide #totp {
                    h2.violet { (icon("shield")) "Authenticator app: on" }
                    p.small { "On since " (since) ". Recovery codes left: " (recovery_left) "." }
                    form.settings method="post" action="/me/totp/disable" {
                        label { "Current password " input type="password" name="current" autocomplete="current-password" required maxlength="200"; }
                        label { "Code from the app, or a recovery code " input type="text" name="code" autocomplete="one-time-code" required maxlength="40"; }
                        button.danger type="submit" { "Turn it off" }
                    }
                    p.muted.small { "Turning it on or off signs out your other sessions." }
                }
            }
        }
    }
}

/// The code step of password sign-in: the password was right, the code
/// comes next. `challenge` is the signed token from the password step.
pub fn login_code(challenge: &str, failed: bool) -> Markup {
    entry::auth_page(
        "Log in",
        html! {
            h1 #auth-h { "One more step" }
            p { "Enter the 6-digit code from your authenticator app, or one of your recovery codes." }
            @if failed { p.alert role="alert" { "That code didn't work. Check and try again; after a few failed tries, each new try waits a little longer." } }
            form.auth-form method="post" action="/login/totp" {
                input type="hidden" name="challenge" value=(challenge);
                label.field for="totp-code" {
                    "Code"
                    input.mono #totp-code name="code" type="text" inputmode="numeric" autocomplete="one-time-code" spellcheck="false" required autofocus maxlength="40";
                }
                button.btn.primary type="submit" { "Log in" }
            }
            a.btn.ghost href="/login" { "Start over" }
        },
    )
}

/// The recovery codes, shown once when the second factor is turned on.
pub fn totp_recovery_codes(codes: &[String]) -> Markup {
    layout(
        "Recovery codes",
        None,
        html! {
            section.login {
                h1 { "Your recovery codes" }
                p { "The authenticator app is on. Keep these codes somewhere safe: each one signs you in once if you lose your phone. They are shown only now." }
                pre.copy { @for c in codes { (c) "\n" } }
                p.muted.small { "Your other sessions are signed out." }
                p { a.button href="/me/sign-in" { "Done" } }
            }
        },
    )
}
