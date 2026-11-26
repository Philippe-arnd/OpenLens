//! Entry pages (#547): log in, the login link, Welcome and the message
//! page. The sign-in pages have no shell: one `.auth-card` on a plain page,
//! with the brand, a heading, one line of text and the form.

use super::*;

/// A page outside the shell: one card in the middle of the screen.
pub(crate) fn auth_page(title: &str, body: Markup) -> Markup {
    let p = prefs();
    html! {
        (DOCTYPE)
        html lang="en" data-theme=[p.theme] {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · OpenLens" }
                link rel="stylesheet" href="/assets/app.css";
                link rel="icon" type="image/svg+xml" href="/assets/favicon.svg";
            }
            body {
                (icons::sprite())
                main.auth {
                    section.card.auth-card aria-labelledby="auth-h" {
                        div.brand { span.brand-mark aria-hidden="true" {} "OpenLens" }
                        (body)
                    }
                    p.auth-foot { "OpenLens " (env!("CARGO_PKG_VERSION")) }
                }
            }
        }
    }
}

/// The admin-token form, the primary action or the way back in (#428).
fn token_form(primary: bool, focus: bool) -> Markup {
    html! {
        form.auth-form method="post" action="/login" {
            label.field for="token" {
                "Admin token"
                input.mono #token name="token" type="password" autocomplete="off" spellcheck="false" required autofocus[focus];
            }
            button.btn.primary[primary] type="submit" { "Open my board" }
        }
    }
}

/// Login page.
pub fn login(failed: bool, sso: bool, email: bool, password: bool) -> Markup {
    auth_page(
        "Log in",
        html! {
            h1 #auth-h { "Log in" }
            @if password {
                p { "Sign in with your email and password." }
                @if failed { p.alert role="alert" { "That didn't work. Check and try again; after a few failed tries, wait 15 minutes." } }
                form.auth-form method="post" action="/login/password" {
                    label.field for="pw-email" {
                        "Email"
                        input #pw-email name="email" type="email" autocomplete="username" required autofocus;
                    }
                    label.field for="pw" {
                        "Password"
                        input #pw name="password" type="password" autocomplete="current-password" required;
                    }
                    button.btn.primary type="submit" { "Log in" }
                }
                // The way back in, always (#428): a lost password, a locked account.
                details.token-login {
                    summary { "Use the admin token" }
                    (token_form(false, false))
                }
            } @else {
                p { "Paste the admin token, or open the login link your agent sent you." }
                @if failed { p.alert role="alert" { "That token didn't work. Try again?" } }
                (token_form(true, true))
            }
            @if sso {
                div.or { "or" }
                a.btn href="/oidc/start" { (icon("shield")) "Sign in with your company account" }
            }
            @if email {
                div.or { "or, if you were invited" }
                form.auth-form method="post" action="/login/email" {
                    label.field for="login-email" {
                        "Your email"
                        input #login-email name="email" type="email" autocomplete="email" required;
                    }
                    button.btn type="submit" { "Email me a login link" }
                }
            }
        },
    )
}

/// The page a login link opens: a button that uses the link. A GET never
/// uses it, so a link preview (Slack, Teams, Outlook Safe Links) leaves it
/// valid for the person it was sent to (#315).
pub fn login_continue(action: &str) -> Markup {
    auth_page(
        "Log in",
        html! {
            h1 #auth-h { "Hello" }
            p { "This link opens your board once. Continue to log in." }
            form.auth-form method="post" action=(action) {
                button.btn.primary type="submit" autofocus { "Continue to my board" }
            }
        },
    )
}

/// First login with the admin token or the agent's link: create your own
/// admin account (named in the team list, and the one SSO signs in to).
pub fn welcome(sso: bool, error: Option<&str>) -> Markup {
    auth_page(
        "Welcome",
        html! {
            h1 #auth-h { "Welcome to OpenLens" }
            p { "You are in with the admin token. Create your own admin account, so the team list knows who you are and you can invite others." }
            @if let Some(e) = error { p.alert role="alert" { (e) } }
            form.auth-form method="post" action="/welcome" {
                label.field for="email" {
                    "Your email"
                    input #email name="email" type="email" autocomplete="email" required autofocus;
                }
                button.btn.primary type="submit" { "Create my admin account" }
            }
            a.btn.ghost href="/" { "Skip for now" }
            p.small {
                @if sso { "Next time, sign in with your company account." }
                @else { "Next time, use a login link from your agent (" code { "openlens-cli login-link --email you@… --role admin" } ") or the admin token." }
            }
        },
    )
}

/// Generic message page (errors, not found).
pub fn message(title: &str, text: &str) -> Markup {
    layout(
        title,
        None,
        html! { section.login { h1 { (title) } p { (text) } p { a href="/" { "Back to the board" } } } },
    )
}

/// [`message`] with markup (a link back, for instance).
pub fn message_html(title: &str, body: Markup) -> Markup {
    layout(
        title,
        None,
        html! { section.login { h1 { (title) } p { (body) } } },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// The opening tag of the element with this `id`.
    fn tag_of<'a>(html: &'a str, id: &str) -> &'a str {
        let at = html.find(&format!("id=\"{id}\"")).unwrap();
        let start = html[..at].rfind('<').unwrap();
        let end = at + html[at..].find('>').unwrap();
        &html[start..=end]
    }

    fn is_emoji(c: char) -> bool {
        matches!(c as u32, 0x2600..=0x27BF | 0x1F000..=0x1FAFF)
    }

    /// No inline style or script (CSP), no emoji, no shell.
    fn plain_entry_page(html: &str) {
        assert!(!html.contains(" style=") && !html.contains("<script"));
        assert!(!html.chars().any(is_emoji), "no emoji");
        assert!(!html.contains("<aside") && !html.contains("side-nav"));
        assert!(html.contains("auth-card"));
    }

    #[test]
    fn entry_pages_are_one_card_without_the_shell() {
        let all = login(true, true, true, true).into_string();
        let token = login(false, false, false, false).into_string();
        for html in [
            &all,
            &token,
            &login_continue("/l/abc").into_string(),
            &welcome(false, Some("nope")).into_string(),
            &welcome(true, None).into_string(),
            &crate::login_code("ch", true).into_string(),
        ] {
            plain_entry_page(html);
        }
        // The token stays hidden on screen, in both variants.
        for html in [&all, &token] {
            assert!(tag_of(html, "token").contains("type=\"password\""));
        }
        // Every variant shares the one card, separated by "or".
        for action in ["/login\"", "/login/password\"", "/login/email\""] {
            assert!(all.contains(&format!("action=\"{action}")), "{action}");
        }
        assert!(all.contains("href=\"/oidc/start\""));
        assert!(all.matches("class=\"or\"").count() >= 2);
    }

    #[test]
    fn the_second_factor_asks_for_a_numeric_one_time_code() {
        let html = crate::login_code("signed", false).into_string();
        let code = tag_of(&html, "totp-code");
        assert!(code.contains("inputmode=\"numeric\""));
        assert!(code.contains("autocomplete=\"one-time-code\""));
        assert!(html.contains("name=\"challenge\" value=\"signed\""));
    }

    #[test]
    fn a_shared_view_says_it_is_read_only() {
        let shared = render_shared(|| message("Gone", "This link expired.")).into_string();
        assert!(shared.contains("class=\"pill") && shared.contains("Read-only view"));
        assert!(
            !shared.contains("<aside")
                && !shared.contains(" style=")
                && !shared.contains("<script")
        );
        let normal = message("Gone", "This link expired.").into_string();
        assert!(!normal.contains("Read-only view"));
    }
}
