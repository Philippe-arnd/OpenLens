//! Storage and backups, tickets, Ask, API tokens and AI settings.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// The backup plan as the page shows it.
pub struct BackupPlan {
    /// `off`, `daily` or `weekly`.
    pub schedule: String,
    /// Hour, UTC.
    pub hour: u8,
    /// 1 (Monday) to 7.
    pub weekday: u8,
    /// Copies kept.
    pub keep: usize,
    /// Folder as set (empty: default).
    pub dir: String,
    /// Folder used.
    pub folder: String,
    /// Also to S3.
    pub to_s3: bool,
}

/// One backup run.
pub struct BackupRun {
    /// When (RFC 3339).
    pub at: String,
    /// Succeeded.
    pub ok: bool,
    /// Files copied.
    pub files: usize,
    /// Size on disk.
    pub bytes: u64,
    /// Duration.
    pub secs: f64,
    /// Local copy.
    pub dest: String,
    /// S3 object, if uploaded.
    pub s3: Option<String>,
    /// Why it failed.
    pub error: Option<String>,
    /// `schedule` or `manual`.
    pub how: String,
}

/// Everything Settings → Storage and backups shows (#218).
pub struct StorageView {
    /// The S3 settings come from `OPENLENS_S3_*` (read-only here).
    pub from_env: bool,
    /// Bucket.
    pub bucket: Option<String>,
    /// Region.
    pub region: Option<String>,
    /// Endpoint.
    pub endpoint: Option<String>,
    /// Prefix.
    pub prefix: Option<String>,
    /// Access key id.
    pub access_key: Option<String>,
    /// A secret is stored (never shown).
    pub secret_set: bool,
    /// The destination is usable (the secret opens).
    pub secret_ok: bool,
    /// Last good copy.
    pub last_ok: Option<String>,
    /// Objects in the last copy.
    pub last_objects: Option<String>,
    /// Last error.
    pub last_error: Option<String>,
    /// Backup plan.
    pub plan: BackupPlan,
    /// Newest first.
    pub history: Vec<BackupRun>,
    /// A backup runs now.
    pub running: bool,
    /// A local copy can be downloaded.
    pub has_latest: bool,
    /// The data folder.
    pub data_dir: String,
    /// The cold tier is on (#417): old days live on S3 only.
    pub cold_on: bool,
    /// Its switch comes from `OPENLENS_S3_COLD`.
    pub cold_from_env: bool,
    /// Why it moves nothing although on.
    pub cold_blocked: Option<String>,
    /// Days whose only copy is on S3 now.
    pub cold_days: usize,
    /// After an action.
    pub notice: Option<String>,
    /// After a failed action.
    pub error: Option<String>,
}

pub(crate) fn bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} kB", n as f64 / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}

/// A drafted ticket (#245).
pub struct TicketDraft<'a> {
    /// Project.
    pub p: &'a ProjectInfo<'a>,
    /// Title.
    pub title: &'a str,
    /// Body, Markdown.
    pub markdown: &'a str,
    /// Finding kind.
    pub kind: &'a str,
    /// Finding subject.
    pub subject: &'a str,
    /// GitHub repository, when set.
    pub repo: Option<String>,
    /// The issue already open for this finding.
    pub existing: Option<String>,
    /// Admins file it.
    pub admin: bool,
}

/// The ticket drafted from a finding (#551): the title, the Markdown body
/// in a monospace field, and the existing actions (open or update the
/// issue on GitHub, or copy it by hand).
pub fn ticket_draft(v: &TicketDraft<'_>) -> Markup {
    let p = v.p;
    layout(
        "New ticket",
        Some(p),
        html! {
            div.heading { div {
                h1 { "New ticket" }
                p.muted { "Prefilled for a person or a coding agent: the problem, the evidence, a hypothesis, the metric OpenLens will check after the release that ships it." }
            } }
            section.card.wide.form-card.ticket-draft aria-labelledby="draft-h" {
                div.section-head {
                    h2 #draft-h { (icon("task")) "Draft" }
                    p { span.tag.tone-info { (v.kind) } }
                }
                div.form-grid {
                    label.field.wide for="t-title" { "Title" input #t-title type="text" readonly value=(v.title); }
                    label.field.wide for="t-body" {
                        "Body (Markdown)"
                        textarea.mono #t-body rows="22" readonly spellcheck="false" { (v.markdown) }
                        small { "Select it and copy it into any tracker." }
                    }
                }
                @match (&v.repo, v.admin) {
                    (Some(repo), true) => form.form-foot method="post" action={ "/p/" (p.id) "/tickets" } {
                        input type="hidden" name="kind" value=(v.kind);
                        input type="hidden" name="subject" value=(v.subject);
                        button.btn.primary type="submit" { @if v.existing.is_some() { "Update the issue with these numbers" } @else { "Open an issue in " (repo) } }
                        @if let Some(u) = &v.existing { a.btn href=(u) { "Open issue" } }
                        p { "OpenLens posts only when you click." }
                    },
                    (None, true) => div.form-foot { p { "To open issues directly, set the GitHub repository on " a href={ "/p/" (p.id) "/tickets" } { "Tickets" } "." } },
                    _ => div.form-foot { p { "An admin can open it as an issue." } },
                }
            }
        },
    )
}

/// One ticket in the list.
pub struct TicketRow {
    /// Title.
    pub title: String,
    /// insight, error, goal or map.
    pub kind: String,
    /// Day created.
    pub created: String,
    /// The issue.
    pub url: Option<String>,
    /// The metric it should move.
    pub metric: Option<String>,
    /// The release that shipped it.
    pub shipped_in: Option<String>,
    /// (verdict, evidence) once measured.
    pub outcome: Option<(String, String)>,
}

/// The tag tone of a ticket's measured effect.
fn effect_tone(verdict: &str) -> &'static str {
    match verdict {
        "improved" => "good",
        "worse" => "bad",
        _ => "warn",
    }
}

/// Tickets and the learning log (#245, #551): an empty state until the
/// first ticket, else the tickets as a table; then the GitHub connection
/// as a form card (admins).
pub fn tickets(
    p: &ProjectInfo<'_>,
    rows: &[TicketRow],
    repo: &str,
    connected: bool,
    admin: bool,
) -> Markup {
    let measured: Vec<&TicketRow> = rows
        .iter()
        .filter(|r| r.outcome.as_ref().is_some_and(|o| o.0 != "no metric"))
        .collect();
    let improved = measured
        .iter()
        .filter(|r| r.outcome.as_ref().is_some_and(|o| o.0 == "improved"))
        .count();
    layout(
        "Tickets",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Tickets" }
                p.muted { "From a finding to a ticket, a release, and the measured effect: OpenLens follows each issue to the release that ships it, measures its metric 14 days later and comments on the issue." }
            } }
            @if !measured.is_empty() {
                // no_detail (#409): the measured tickets are listed below.
                section.tiles aria-label="Learning" {
                    (tile("mint", "star", "Changes that moved their metric", &format!("{improved} of {}", measured.len()), "measured 14 days after their release"))
                }
            }
            @if rows.is_empty() {
                section.card.empty aria-labelledby="no-tickets" {
                    (crate::tables::empty_art())
                    div {
                        h2 #no-tickets { "No ticket yet" }
                        p { "Use " em { "Create ticket" } " on an insight card, an error, a goal or a feature map finding. The draft holds the evidence and the metric to check; OpenLens then follows it to the release that ships it." }
                        div.actions {
                            a.btn.primary href={ "/p/" (p.id) } { "Open the Focus board" }
                            a.btn href={ "/p/" (p.id) "/errors" } { "Errors" }
                        }
                    }
                }
            } @else {
                (crate::tables::TableCard {
                    label: "Tickets",
                    head: html! { h2 { (icon("task")) "Tickets" } },
                    cols: &[
                        crate::tables::Col::Shown("Ticket"), crate::tables::Col::Shown("From"),
                        crate::tables::Col::Shown("Metric"), crate::tables::Col::Shown("Shipped in"),
                        crate::tables::Col::Shown("Effect"),
                    ],
                    body: html! { @for r in rows {
                        tr {
                            th scope="row" { @match &r.url { Some(u) => a href=(u) { (r.title) }, None => (r.title) } br; span.muted.small { (r.created) } }
                            td { span.tag.tone-info { (r.kind) } }
                            td.small { (r.metric.as_deref().unwrap_or("—")) }
                            td.small { @match &r.shipped_in { Some(s) => code { (s) }, None => span.muted { "not yet" } } }
                            td.small { @match &r.outcome {
                                Some((w, ev)) => { span class={ "tag tone-" (effect_tone(w)) } { (w) } br; span.muted { (ev) } },
                                None => span.muted { "waiting" },
                            } }
                        }
                    } },
                    foot: html! {},
                }
                .render())
            }
            @if admin {
                form.card.wide.form-card method="post" action={ "/p/" (p.id) "/tickets/target" } aria-labelledby="github-h" {
                    div.section-head {
                        h2 #github-h { (icon("code")) "GitHub" }
                        p { @if connected { span.tag.tone-good { (icon("task")) "token stored" } } @else { span.tag { "no token" } } }
                    }
                    div.form-grid {
                        label.field { "Repository" input type="text" name="repo" value=(repo) placeholder="owner/name" maxlength="140"; small { "Where issues are opened, as owner/name." } }
                        label.field { "Token"
                            input type="password" name="token" autocomplete="off" placeholder=(if connected { "stored (leave empty to keep it)" } else { "fine-grained, Issues: read and write" });
                            small { "Stored encrypted (or set " code { "OPENLENS_GITHUB_TOKEN" } ") and shared by every project." }
                        }
                    }
                    (crate::project_settings::form_foot("Save", html! { "OpenLens only posts when you click, and comments on issues it opened. Without it, copy the drafted Markdown into any tracker." }))
                }
            }
        },
    )
}

/// One question and its answer in Ask (#244), as kept: no raw tool results.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AskTurn {
    /// When (RFC 3339).
    pub at: String,
    /// The question.
    pub question: String,
    /// The answer.
    pub answer: String,
    /// (label, link or query) behind the answer.
    #[serde(default)]
    pub evidence: Vec<(String, String)>,
    /// Tokens used.
    #[serde(default)]
    pub tokens: i64,
    /// Of them, read from the provider's cache.
    #[serde(default)]
    pub cached: i64,
    /// A change waiting for confirmation: (token, plain description).
    #[serde(default)]
    pub pending: Option<(String, String)>,
}

/// What the Ask page shows (#244).
pub struct AskView<'a> {
    /// Project.
    pub p: &'a ProjectInfo<'a>,
    /// Provider and model when Ask is on.
    pub on: Option<String>,
    /// My conversation, oldest first.
    pub turns: &'a [AskTurn],
    /// The page I came from.
    pub from: Option<&'a str>,
    /// An error to show.
    pub error: Option<&'a str>,
    /// Tokens used today in the project.
    pub used: i64,
    /// Daily budget.
    pub budget: i64,
    /// Admins confirm changes.
    pub admin: bool,
}

/// Ask OpenLens (#244): the conversation and the question box.
pub fn ask(v: &AskView<'_>) -> Markup {
    let p = v.p;
    let n = v.turns.len();
    layout(
        "Ask",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Ask" }
                p.muted { "Ask about this project in your words. The answer comes with the numbers and the pages behind it. Changes are only proposed: an admin confirms them." }
            } }
            @if let Some(e) = v.error { p.alert role="alert" { (e) } }
            @match &v.on {
                None => section.card.wide {
                    p { "Ask is off. An admin turns it on in " a href="/settings/ai" { "Settings → AI" } ": an Anthropic key, or any OpenAI-compatible server (a LiteLLM proxy, OpenRouter, a local Ollama: then no data leaves your network)." }
                },
                Some(on) => {
                    @for (i, t) in v.turns.iter().enumerate() {
                        section.card.wide.turn id=[(i + 1 == n).then_some("last")] {
                            p.question { strong { (t.question) } span.muted.small { " · " (t.at.get(..16).unwrap_or(&t.at).replace('T', " ")) } }
                            div.answer {
                                @for para in t.answer.split("\n\n") {
                                    @let lines: Vec<&str> = para.lines().collect();
                                    @if lines.iter().all(|l| l.trim_start().starts_with("- ")) && !lines.is_empty() {
                                        ul { @for l in &lines { li { (l.trim_start().trim_start_matches("- ")) } } }
                                    } @else if lines.iter().all(|l| l.starts_with("? ")) && !lines.is_empty() {
                                        ul.follow { @for l in &lines { li { form.inline method="post" action={ "/p/" (p.id) "/ask" } {
                                            input type="hidden" name="question" value=(l.trim_start_matches("? "));
                                            button.ghost type="submit" { (l.trim_start_matches("? ")) }
                                        } } } }
                                    } @else {
                                        p { @for (j, l) in lines.iter().enumerate() { @if j > 0 { br; } (l) } }
                                    }
                                }
                            }
                            @if let Some((token, desc)) = &t.pending {
                                div.pending {
                                    p { strong { "Proposed change" } }
                                    pre.diff { ins { "+ " (desc) } }
                                    @if v.admin {
                                        form.inline method="post" action={ "/p/" (p.id) "/ask/confirm" } {
                                            input type="hidden" name="token" value=(token);
                                            button type="submit" { "Apply this change" }
                                        }
                                        span.muted.small { " Nothing changes until you click. The proposal expires in 30 minutes." }
                                    } @else { p.muted.small { "An admin can apply it." } }
                                }
                            }
                            @if !t.evidence.is_empty() {
                                details { summary.small { "Evidence (" (t.evidence.len()) ")" }
                                    ul.plain-list.small { @for (label, e) in &t.evidence {
                                        li { @if e.starts_with('/') { a href=(e) { (label) } } @else { (label) ": " code { (e) } } }
                                    } }
                                }
                            }
                            p.muted.small { (t.tokens) " tokens" @if t.cached > 0 { ", " (t.cached) " from the cache" } }
                        }
                    }
                    section.card.wide {
                        form.sqlbox method="post" action={ "/p/" (p.id) "/ask" } {
                            label for="ask-q" { "Your question" @if let Some(f) = v.from { span.muted.small { " · about " code { (f) } } } }
                            textarea #ask-q name="question" rows="3" required maxlength="2000" placeholder="Why did writing drop in the last release?" {}
                            @if let Some(f) = v.from { input type="hidden" name="from" value=(f); }
                            div.actions {
                                label.check { input type="checkbox" name="deep"; " Investigate deeply (main model)" }
                                button type="submit" { "Ask" }
                            }
                        }
                        p.muted.small {
                            (on) " · today " (compact(v.used)) " of " (compact(v.budget)) " tokens · "
                            a href={ "/p/" (p.id) "/ask/export.md" } { "Export" }
                            @if v.admin { " · " a href={ "/p/" (p.id) "/ask/sent" } { "What is sent" } }
                        }
                        p.muted.small { "The model sees the project's aggregates and definitions, never event properties, replays or emails (unless an admin allows person-level detail). Your conversation is kept 30 days." }
                        form.inline method="post" action={ "/p/" (p.id) "/ask/clear" } { button.ghost type="submit" { "Forget this conversation" } }
                        @if v.admin {
                            form.inline method="post" action={ "/p/" (p.id) "/ask/privacy" } {
                                label.check { input type="checkbox" name="person_level"; " Allow person-level detail (user ids in SQL) for this project" }
                                " " button.ghost type="submit" { "Save" }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// One scoped API token in Settings → API tokens (#342), never its value.
pub struct TokenRow {
    /// Id, to revoke it.
    pub id: String,
    /// What it is for.
    pub label: String,
    /// `read`, `write` or `admin`.
    pub scope: String,
    /// The projects it reaches (`None`: all).
    pub projects: Option<Vec<String>>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub last_used: Option<String>,
}

/// Everything Settings → API tokens shows (#342).
pub struct TokensView {
    /// The tokens.
    pub tokens: Vec<TokenRow>,
    /// Every project, `(id, name)`, to limit a new token.
    pub projects: Vec<(String, String)>,
    /// A token just created: shown once, here.
    pub created: Option<String>,
    /// What just happened.
    pub notice: Option<String>,
    /// What went wrong.
    pub error: Option<String>,
}

/// Settings → API tokens and sessions (#342).
pub fn tokens(v: &TokensView) -> Markup {
    let when = |t: &str| t.get(..16).unwrap_or(t).replace('T', " ");
    layout(
        "API tokens",
        None,
        html! {
            div.heading { div {
                h1 { "API tokens" }
                p.muted { "Tokens for the CLI, MCP and scripts. " code { "read" } " reads (enough for " code { "openlens-cli mcp" } "), " code { "write" } " also changes projects, " code { "admin" } " does everything the admin token does. A token never opens dashboard pages." }
            } }
            @if let Some(n) = &v.notice { p.notice role="status" { (n) } }
            @if let Some(e) = &v.error { p.alert role="alert" { (e) } }
            @if let Some(t) = &v.created {
                section.card.wide {
                    h2.mint { (icon("task")) "Your new token" }
                    p { "Copy it now: it is stored hashed and will not be shown again." }
                    pre.snippet { code { (t) } }
                }
            }
            form.card.wide.form-card method="post" action="/settings/tokens" aria-labelledby="token-h" {
                div.section-head { h2 #token-h { (icon("code")) "Create a token" } }
                div.form-grid {
                    label.field { "Label" input type="text" name="label" maxlength="100" placeholder="what it is for" required; small { "Shown in the table below, never the token itself." } }
                    label.field { "Scope"
                        select name="scope" {
                            option value="read" { "read" }
                            option value="write" { "write" }
                            option value="admin" { "admin" }
                        }
                        small { "read is enough for " code { "openlens-cli mcp" } "." }
                    }
                    @if !v.projects.is_empty() {
                        fieldset.field.wide {
                            legend { "Projects (none ticked: all)" }
                            @for (id, name) in &v.projects {
                                label.check { input type="checkbox" name="project" value=(id); " " (name) }
                            }
                        }
                    }
                }
                (crate::project_settings::form_foot("Create the token", html! { "The token is shown once, then stored hashed." }))
            }
            (crate::tables::TableCard {
                label: "Tokens",
                head: html! { h2 { (icon("shield")) "Tokens" } },
                cols: &[
                    crate::tables::Col::Shown("Label"), crate::tables::Col::Shown("Scope"),
                    crate::tables::Col::Shown("Projects"), crate::tables::Col::Shown("Created"),
                    crate::tables::Col::Shown("Last used"), crate::tables::Col::Hidden("Actions"),
                ],
                body: html! {
                    @if v.tokens.is_empty() { tr { td.muted colspan="6" { "No token yet." } } }
                    @for t in &v.tokens {
                        tr {
                            td { (t.label) br; span.small.muted.mono { (t.id) } }
                            td { span class={ "tag tone-" (match t.scope.as_str() { "admin" => "bad", "write" => "warn", _ => "info" }) } { (t.scope) } }
                            td.small { @match &t.projects { Some(p) => (p.join(", ")), None => span.muted { "all" } } }
                            td.when.muted { (when(&t.created_at)) }
                            td.when.muted { (t.last_used.as_deref().map(when).unwrap_or_else(|| "never".into())) }
                            td { form method="post" action={ "/settings/tokens/" (t.id) "/revoke" } { button.ghost type="submit" { "Revoke" } } }
                        }
                    }
                },
                foot: html! {},
            }
            .render())
            section.card.wide {
                h2.coral { (icon("users")) "Sessions" }
                p { "Signs out every browser that is logged in, yours excepted: people log in again with a login link, SSO or the admin token. API tokens and the admin token keep working." }
                form method="post" action="/settings/sessions/revoke" { button type="submit" { "Sign out everywhere" } }
            }
        },
    )
}

/// Settings → AI (#244).
pub struct AiView<'a> {
    /// Env vars set it (the form is read-only then).
    pub from_env: bool,
    /// The preset the stored settings match (`custom`, or empty when off).
    pub preset: String,
    /// The presets offered (#406).
    pub presets: Vec<AiPreset>,
    /// The models the provider listed at the last save.
    pub models: Vec<String>,
    /// What runs now.
    pub active: Option<String>,
    /// Stored values.
    pub provider: String,
    /// Stored base URL.
    pub base_url: String,
    /// Stored model.
    pub model: String,
    /// Stored fast model.
    pub fast_model: String,
    /// A key is stored (never shown).
    pub key_set: bool,
    /// Daily tokens per project.
    pub budget: i64,
    /// Notice.
    pub notice: Option<&'a str>,
    /// Error.
    pub error: Option<&'a str>,
}

/// One provider preset in the picker (#406).
pub struct AiPreset {
    /// Form value.
    pub id: &'static str,
    /// Shown name.
    pub label: &'static str,
    /// What a key looks like.
    pub key_hint: &'static str,
    /// Where to get a key.
    pub key_url: &'static str,
}

/// How the key and the token budget are handled, under the AI form.
fn ai_terms(budget: i64) -> Markup {
    html! {
        "The key is stored encrypted and never shown again; it only travels in a header to the provider. Each project may use " (compact(budget)) " tokens a day (" code { "OPENLENS_LLM_DAILY_TOKENS" } "), and each person 30 questions an hour. See docs/ai.md for LiteLLM and Ollama."
    }
}

/// The AI settings page.
pub fn ai_settings(v: &AiView<'_>) -> Markup {
    layout(
        "AI",
        None,
        html! {
            div.heading { div {
                h1 { "AI" }
                p.muted { "The model behind Ask. One setting covers every mode: Anthropic, or any server that speaks the OpenAI Chat Completions API (OpenAI, a LiteLLM proxy, OpenRouter, Azure OpenAI, Mistral, vLLM, a local Ollama)." }
            } }
            @if let Some(n) = v.notice { p.notice role="status" { (n) } }
            @if let Some(e) = v.error { p.alert role="alert" { (e) } }
            @if v.from_env {
                section.card.wide {
                    h2.violet { (icon("spark")) "Model" }
                    p { "Now: " strong { (v.active.as_deref().unwrap_or("off")) } }
                    p.muted { "Set by the " code { "OPENLENS_LLM_*" } " environment variables, which win over this page." }
                    p.muted.small { (ai_terms(v.budget)) }
                }
            } @else {
                // The provider stays a select: there are more presets than a
                // segmented control holds. The key hint follows the chosen
                // preset through :has() (no JavaScript).
                form.card.wide.form-card.ai-form method="post" action="/settings/ai" aria-labelledby="ai-h" {
                    div.section-head {
                        h2 #ai-h { (icon("spark")) "Model" }
                        p { "Now: " strong { (v.active.as_deref().unwrap_or("off")) } }
                    }
                    div.form-grid {
                        label.field { "Provider"
                            select name="preset" {
                                option value="off" selected[v.preset.is_empty()] { "off" }
                                @for p in &v.presets {
                                    option value=(p.id) selected[v.preset == p.id] { (p.label) }
                                }
                                option value="custom" selected[v.preset == "custom"] { "Custom (any OpenAI-compatible server, Azure, LiteLLM, vLLM…)" }
                            }
                            small { "Off: Ask is turned off." }
                        }
                        label.field { "API key"
                            input type="password" name="key" autocomplete="off" placeholder=(if v.key_set { "stored (leave empty to keep it)".to_owned() } else { "paste it here".to_owned() });
                            small.key-help {
                                @for p in &v.presets {
                                    span class={ "for-" (p.id) } { (p.label) " keys look like " code { (p.key_hint) } " · " a href=(p.key_url) rel="noopener" target="_blank" { "get one" } }
                                }
                            }
                        }
                        div.custom-only {
                            label.field { "Protocol"
                                select name="provider" {
                                    option value="openai-compatible" selected[v.provider != "anthropic"] { "OpenAI Chat Completions" }
                                    option value="anthropic" selected[v.provider == "anthropic"] { "Anthropic Messages" }
                                }
                                small { "What the server speaks." }
                            }
                        }
                        div.url-field {
                            label.field { "Base URL"
                                input type="url" name="base_url" value=(v.base_url) placeholder="empty: the preset's address · http://ollama:11434/v1 · http://litellm:4000/v1" maxlength="300";
                                small { "Empty: the preset's address." }
                            }
                        }
                    }
                    details.models open[v.preset == "custom"] {
                        summary { "Models" @if !v.model.is_empty() { " · " (v.model) " / " (if v.fast_model.is_empty() { v.model.as_str() } else { v.fast_model.as_str() }) } }
                        datalist id="ai-models" { @for m in &v.models { option value=(m) {} } }
                        div.form-grid {
                            label.field { "Model" input type="text" name="model" list="ai-models" value=(v.model) placeholder="picked from the provider's list" maxlength="120"; small { "For questions and investigations." } }
                            label.field { "Fast model" input type="text" name="fast_model" list="ai-models" value=(v.fast_model) placeholder="for short lookups" maxlength="120"; small { "For short lookups." } }
                        }
                    }
                    p.muted.small { (ai_terms(v.budget)) }
                    div.form-foot {
                        button.btn.primary type="submit" name="action" value="save" { "Save and check" }
                        button.btn type="submit" name="action" value="test" { "Test" }
                        @if v.key_set { button.btn.ghost type="submit" name="action" value="forget" { "Remove the key" } }
                        p { "Test sends one short request to the provider to check the key." }
                    }
                }
            }
        },
    )
}

/// The data directory as a page may print it (#551): its last component
/// only, since the full path can name a home directory.
pub(crate) fn data_dir_name(dir: &str) -> String {
    std::path::Path::new(dir)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .map_or_else(|| "the data directory".to_owned(), ToOwned::to_owned)
}

/// A local path as a page may print it (#551): inside the data directory,
/// from the directory's last component on; elsewhere, its last component
/// only. Never the full path.
pub(crate) fn shown_path(path: &str, data_dir: &str) -> String {
    let p = std::path::Path::new(path);
    if !data_dir.is_empty()
        && let Ok(rest) = p.strip_prefix(data_dir)
    {
        let base = data_dir_name(data_dir);
        return if rest.as_os_str().is_empty() {
            base
        } else {
            format!("{base}/{}", rest.display())
        };
    }
    match p.file_name().and_then(|n| n.to_str()) {
        Some(name) if p.components().count() > 1 => format!("…/{name}"),
        Some(name) => name.to_owned(),
        None => "—".to_owned(),
    }
}

/// Settings → Storage and backups (#218, #551): the S3 copy as a form
/// card, the cold tier in its own card, the backup plan as a form card
/// and its runs as a table, then how to restore. Local paths are printed
/// from the data directory's name on, never in full.
pub fn storage(v: &StorageView) -> Markup {
    let ro = v.from_env;
    let dir = data_dir_name(&v.data_dir);
    let folder = shown_path(&v.plan.folder, &v.data_dir);
    let day = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    layout(
        "Storage and backups",
        None,
        html! {
            div.heading { div {
                h1 { "Storage and backups" }
                p.muted { "The local data directory (" code { (dir) } ") holds your data. S3 keeps a copy of closed days, and, only if you turn the cold tier on, becomes the only copy of days older than 120 days. Backups are point-in-time copies you can restore." }
            } }
            @if v.cold_days > 0 {
                p.alert role="status" { strong { (v.cold_days) " event days live only on S3." } " Losing the bucket, or a lifecycle rule that expires its objects, loses them. Keep the bucket and its versioning." }
            }
            @if let Some(n) = &v.notice { p.notice role="status" { (n) } }
            @if let Some(e) = &v.error { p.alert role="alert" { (e) } }
            form.card.wide.form-card method="post" action="/settings/storage/s3" aria-labelledby="s3-h" {
                div.section-head {
                    h2 #s3-h { (icon("layers")) "S3-compatible copy" }
                    p {
                        @match &v.last_ok {
                            Some(t) => { span.tag.tone-good { (icon("task")) "Last copy " (t.get(..16).unwrap_or(t).replace('T', " ")) " UTC" } @if let Some(n) = &v.last_objects { " " (n) " objects" } },
                            None => span.tag { "No copy yet" },
                        }
                    }
                }
                @if ro { p.small { span.tag.tone-info { "set by environment" } " The " code { "OPENLENS_S3_*" } " variables win; change them where the server runs." } }
                div.form-grid {
                    label.field { "Bucket" input type="text" name="bucket" value=[v.bucket.as_deref()] maxlength="63" disabled[ro] required; }
                    label.field { "Region" input type="text" name="region" value=[v.region.as_deref()] placeholder="us-east-1" maxlength="40" disabled[ro]; }
                    label.field { "Endpoint" input type="url" name="endpoint" value=[v.endpoint.as_deref()] placeholder="AWS: empty · R2, MinIO, Garage: https://…" maxlength="200" disabled[ro]; small { "Empty for AWS." } }
                    label.field { "Prefix" input type="text" name="prefix" value=[v.prefix.as_deref()] placeholder="openlens" maxlength="100" disabled[ro]; }
                    label.field { "Access key" input type="text" name="access_key" value=[v.access_key.as_deref()] maxlength="200" autocomplete="off" disabled[ro] required; }
                    label.field { "Secret key" input type="password" name="secret" placeholder=(if v.secret_set { "stored · leave empty to keep" } else { "" }) maxlength="200" autocomplete="new-password" disabled[ro]; small { "Stored sealed and never shown again." } }
                }
                @if v.secret_set && !v.secret_ok && !ro { p.alert role="alert" { "The stored secret can't be opened (the server's secret key changed, or the store was restored): enter it again." } }
                @if let Some(e) = &v.last_error { p.small { span.tag.tone-bad { (icon("alert")) "Last error" } " " (e) } }
                div.form-foot {
                    @if !ro { button.btn.primary type="submit" name="action" value="save" { "Save" } }
                    @if v.bucket.is_some() { button.btn type="submit" name="action" value="test" formnovalidate { "Test the connection" } }
                    @if !ro && v.bucket.is_some() { button.btn.ghost type="submit" name="action" value="remove" formnovalidate { "Remove" } }
                    p { "Copied hourly: every closed day of events, and a daily snapshot of the metadata." }
                }
            }
            section.card.wide.form-card #cold aria-labelledby="cold-h" {
                div.section-head {
                    h2 #cold-h { (icon("layers")) "Cold tier: old days on S3 only" }
                    p { @if v.cold_on { span.tag.tone-warn { "on" } } @else { span.tag { "off" } } }
                }
                p.small { "Off by default. When on, event days older than 120 days leave the disk once S3 holds every file of them, checked. S3 then becomes their " strong { "only" } " copy (D-031). OpenLens first reads the bucket's lifecycle rules and moves nothing if one expires objects in under 400 days, or if they cannot be read." }
                @if let Some(b) = &v.cold_blocked { p.alert role="alert" { "Nothing moves: " (b) "." } }
                @if v.cold_from_env {
                    p.small { span.tag.tone-info { "set by environment" } " " code { "OPENLENS_S3_COLD" } " is " strong { (if v.cold_on { "on" } else { "off" }) } "." }
                } @else {
                    form.form-body method="post" action="/settings/storage/cold" {
                        label.switch {
                            input type="checkbox" role="switch" name="on" checked[v.cold_on];
                            span { b { "Move days older than 120 days to S3 and remove them from the disk" } small { "S3 becomes their only copy." } }
                        }
                        div.form-foot { button.btn.primary type="submit" { "Save" } }
                    }
                }
            }
            form.card.wide.form-card method="post" action="/settings/storage/backups" aria-labelledby="backups-h" {
                div.section-head { h2 #backups-h { (icon("clock")) "Backups" } }
                div.form-grid {
                    label.field { "When"
                        select name="schedule" {
                            @for (k, l) in [("off", "Off"), ("daily", "Every day"), ("weekly", "Every week")] {
                                option value=(k) selected[v.plan.schedule == k] { (l) }
                            }
                        }
                    }
                    label.field { "At (UTC)" select name="hour" { @for h in 0u8..24 { option value=(h) selected[v.plan.hour == h] { (format!("{h:02}:00")) } } } }
                    label.field { "On (weekly)" select name="weekday" { @for (i, d) in day.iter().enumerate() { option value=(i + 1) selected[usize::from(v.plan.weekday) == i + 1] { (d) } } } }
                    label.field { "Keep" input type="number" name="keep" min="1" max="60" value=(v.plan.keep); small { "Copies, the oldest removed first." } }
                    label.field { "Folder" input type="text" name="dir" value=(v.plan.dir) placeholder=(folder) maxlength="300"; small { "Empty: " code { (folder) } "." } }
                }
                label.switch {
                    input type="checkbox" role="switch" name="to_s3" checked[v.plan.to_s3];
                    span { b { "Also to S3 (.tar.gz)" } small { "Uses the S3 copy's bucket." } }
                }
                (crate::project_settings::form_foot("Save", html! { "Each copy is consistent while the server runs and takes as much room as the data. Failures are emailed to admins when SMTP is set." }))
            }
            div.actions.backup-actions {
                form.inline method="post" action="/settings/storage/backups/run" {
                    button type="submit" disabled[v.running] { (if v.running { "Backing up…" } else { "Back up now" }) }
                }
                @if v.has_latest { a.button href="/settings/storage/backups/latest.tar.gz" { (icon("down")) "Download the latest" } }
            }
            @if !v.history.is_empty() {
                (crate::tables::TableCard {
                    label: "Backup runs",
                    head: html! { h2 { (icon("clock")) "Backup runs" } },
                    cols: &[
                        crate::tables::Col::Shown("When (UTC)"), crate::tables::Col::Shown("Result"),
                        crate::tables::Col::Num("Size"), crate::tables::Col::Num("Files"),
                        crate::tables::Col::Num("Took"), crate::tables::Col::Shown("Where"),
                    ],
                    body: html! { @for r in &v.history {
                        tr {
                            td.when { (r.at.get(..16).unwrap_or(&r.at).replace('T', " ")) " " span.muted { (r.how) } }
                            td { @if r.ok { span.tag.tone-good { (icon("task")) "ok" } } @else { span.tag.tone-bad { (icon("alert")) "failed" } br; span.small { (r.error.as_deref().unwrap_or("")) } } }
                            td.num { (bytes(r.bytes)) }
                            td.num { (r.files) }
                            td.num { (duration(r.secs)) }
                            td.path { (shown_path(&r.dest, &v.data_dir)) @if let Some(k) = &r.s3 { br; "s3: " (k) } }
                        }
                    } },
                    foot: html! {},
                }
                .render())
            }
            section.card.wide {
                h2.coral { (icon("shield")) "Restore" }
                p { "Restoring replaces the data, so it is done with the server stopped. Below, " code { "<data>" } " is the data directory (" code { (dir) } ", " code { "OPENLENS_DATA_DIR" } " where the server runs):" }
                pre.snippet { code { "# stop OpenLens, then
mv <data> <data>.old
cp -a <backups>/<copy> <data>
# start OpenLens" } }
                p.muted.small { code { "<backups>" } " is the backup folder (" code { (folder) } "). From S3: download " code { "backups/<copy>.tar.gz" } ", " code { "tar xzf" } " it and use the folder inside. S3 secrets are not in backups: enter them again after a restore." }
            }
        },
    )
}
