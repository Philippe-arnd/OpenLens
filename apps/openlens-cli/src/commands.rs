//! The subcommands and their arguments. Moved out of main.rs as is (#307).

use super::*;

#[derive(Subcommand)]
pub(super) enum TokensCmd {
    /// Every scoped API token (never their values).
    List,
    /// Make a token for an agent or a script; it is printed once.
    Create {
        /// read, write or admin.
        #[arg(long)]
        scope: String,
        /// Limit it to this project (repeat for several).
        #[arg(long = "project")]
        projects: Vec<String>,
        /// What it is for.
        #[arg(long, default_value = "")]
        label: String,
    },
    /// Revoke a token (id from `tokens list`).
    Revoke { id: String },
}

#[derive(Subcommand)]
pub(super) enum EnvCmd {
    /// Hosts per environment (last 30 days), the host rules, and the
    /// hosts no rule classifies yet (stored as `unknown`).
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Say which environment a host is: `app.example.com production`,
    /// `*.preview.example.com preview`, or `ignore` to drop its events.
    /// Applies to the next events, with no change to the app.
    Rule {
        /// Exact host or `*.suffix`.
        host: String,
        /// Environment name, or `ignore`.
        env: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Move a host's past events (exact or `*.suffix`) to the environment
    /// the rules give now, after a rule changed. `--dry-run` only counts
    /// the moves per old and new environment.
    Relabel {
        /// Exact host or `*.suffix`.
        host: String,
        /// Days back, today included (1 to 90).
        #[arg(long, default_value_t = 30)]
        days: i64,
        /// Count what would move, write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Accept events only from these hosts (exact or `*.suffix`; none
    /// turns it off). Shows what the list would have dropped over the last
    /// 24 hours; `--confirm` saves it. It drops nothing until a listed host
    /// has sent events in the last 7 days, and never drops events without
    /// a page address.
    Allow {
        /// Exact hosts or `*.suffix`, at most 50.
        hosts: Vec<String>,
        /// Save the list (without it, only the preview is shown).
        #[arg(long)]
        confirm: bool,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Remove the rule for a host.
    Unrule {
        /// The host of the rule.
        host: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Store an environment the app sends under another name:
    /// `live-eu production`, or `sandbox ignore` to drop its events.
    Alias {
        /// The name the app sends.
        from: String,
        /// The environment to store it as, or `ignore`.
        to: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Remove an alias.
    Unalias {
        /// The name the alias renames.
        from: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Hide an environment from the selector (its events are kept and
    /// still readable by name).
    Hide {
        /// The environment.
        env: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Show a hidden environment again.
    Show {
        /// The environment.
        env: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Check a non-production environment at its own address every
    /// minute (and its certificate), with its own alert level.
    Monitor {
        /// The environment (not production: its address is the app
        /// address).
        env: String,
        /// `https://staging.example.com`.
        url: String,
        /// `log` (recorded on Uptime only) or `notify` (announced).
        #[arg(long, default_value = "log")]
        level: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Stop checking an environment.
    Unmonitor {
        /// The environment.
        env: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

#[derive(Subcommand)]
pub(super) enum UsersCmd {
    /// Every user, with role, projects, time zone and last sign-in.
    List,
    /// Change a user's name, email, role, projects or time zone.
    Update {
        /// User id (from `users list`).
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        email: Option<String>,
        /// viewer or admin.
        #[arg(long)]
        role: Option<String>,
        /// Comma-separated project ids, or `all`.
        #[arg(long)]
        projects: Option<String>,
        /// IANA zone they read times in (empty: the project's).
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Refuse sign-in (their sessions end at once).
    Disable { id: String },
    /// Allow sign-in again.
    Enable { id: String },
    /// Remove a user (never the last admin).
    Delete { id: String },
}

#[derive(Subcommand)]
pub(super) enum Command {
    /// Create a project; prints its capture key, the snippet to add and
    /// the steps that keep it running (deploy hook URL, release step,
    /// rules).
    Init {
        /// Project name (usually the app's name).
        #[arg(long)]
        name: String,
        /// The team's IANA time zone (`Europe/Paris`): days and weeks of
        /// every KPI are counted in it. UTC when omitted.
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Set a project's time zone (IANA name, e.g. `Europe/Paris`).
    Timezone {
        /// Project id.
        project: String,
        /// IANA time zone name.
        timezone: String,
    },
    /// Delete a project and all its data (events, replays, people, flags,
    /// surveys, feature map). Irreversible: repeat the id with --confirm.
    DeleteProject {
        /// Project id.
        id: String,
        /// The same id again.
        #[arg(long)]
        confirm: String,
    },
    /// Feature map commands.
    Features {
        #[command(subcommand)]
        action: FeaturesCmd,
    },
    /// Check the wiring: events arrive, roles resolve, features fire; and
    /// list the release layers that are active.
    Verify {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Backfill history from the app's own data (JSONL: persons and past events).
    Import {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// JSONL file. Person lines: {"type":"person","id","role","account","plan","created_at","last_seen_at"}.
        /// Event lines: {"event","distinct_id","timestamp","properties"}.
        #[arg(long)]
        file: PathBuf,
        /// Another tool's per-event export: `csv` or `jsonl` with its own
        /// column names (see --map). Without it, the file is OpenLens JSONL.
        #[arg(long)]
        format: Option<String>,
        /// Columns of the export: `event=…,distinct_id=…,timestamp=…`, and
        /// optionally `url=…,referrer=…,session_id=…`. Other columns become
        /// properties.
        #[arg(long, default_value = "")]
        map: String,
    },
    /// Export raw events as JSONL, day by day, for deep analysis.
    Export {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// First day, YYYY-MM-DD.
        #[arg(long)]
        from: String,
        /// Last day, YYYY-MM-DD (inclusive).
        #[arg(long)]
        to: String,
        /// Output file (JSONL).
        #[arg(long)]
        out: PathBuf,
    },
    /// Micro-surveys: list (with results), set (create or update) and delete.
    Surveys {
        #[command(subcommand)]
        action: SurveysCmd,
    },
    /// Feature flags: list, set (create or update) and delete.
    Flags {
        #[command(subcommand)]
        action: FlagsCmd,
    },
    /// Add a note to the timeline (for example right after a deploy);
    /// without --note, list the timeline.
    Annotate {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// The note (1–500 characters).
        #[arg(long)]
        note: Option<String>,
        /// Its day (YYYY-MM-DD, default today).
        #[arg(long)]
        day: Option<String>,
    },
    /// Goals on KPIs: each with its target, current value, trend at the
    /// deadline and status, plus a suggested target per KPI.
    Goals {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Status announcements and incident updates (#562): list, post,
    /// update, edit, close, delete, and whether they go to the alert
    /// channels.
    Announce {
        #[command(subcommand)]
        action: AnnounceCmd,
    },
    /// The public status page (#561): show or change its settings (on or
    /// off, the address, the components and their public names), or read
    /// the page as anyone sees it.
    StatusPage {
        #[command(subcommand)]
        action: crate::status_page::StatusPageCmd,
    },
    /// Is the app up? (`up`, `down`, `maintenance`, `unknown`, `off`, with
    /// open incidents and maintenance windows.) Exits non-zero when down:
    /// handy right after a deploy.
    Status {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Uptime history, one row per UTC day (default: the last 90 days of
    /// production): state, availability, seconds up and measured, checks,
    /// incidents and response times.
    Uptime {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// First day, YYYY-MM-DD (at most 400 days before --to).
        #[arg(long)]
        from: Option<String>,
        /// Last day, YYYY-MM-DD (default: today).
        #[arg(long)]
        to: Option<String>,
        /// Environment (default: production).
        #[arg(long)]
        env: Option<String>,
    },
    /// Availability over 24 hours, 7, 30, 90 and 365 days, this month and
    /// the last (#560): time not measured and planned maintenance left
    /// out, incidents, p95; and the error budget when a target is set.
    /// With --target (a share, 0.999 for 99.9%) or --clear, sets or
    /// removes the environment's target over a rolling 30 days.
    Availability {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Environment (default: production).
        #[arg(long)]
        env: Option<String>,
        /// Set the target: a share from 0.5 to 0.99999.
        #[arg(long, conflicts_with = "clear")]
        target: Option<f64>,
        /// Remove the target.
        #[arg(long)]
        clear: bool,
    },
    /// Every incident, newest first, page by page: pass the `next` it
    /// prints as --before for the next page.
    Incidents {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Only incidents older than this id (the previous page's `next`).
        #[arg(long)]
        before: Option<i64>,
        /// Page size, 1 to 100.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Planned maintenance windows (#563): incidents during one are not
    /// announced and its time is not counted against availability.
    Maintenance {
        #[command(subcommand)]
        action: crate::maintenance::MaintenanceCmd,
    },
    /// What users said (#718): feedback per rating and feature over
    /// --days (1 to 90, default 30), with the latest texts and who sent
    /// them. Texts older than 90 days are gone; the ratings stay.
    Feedback {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// One feature key of the map.
        #[arg(long)]
        feature: Option<String>,
        /// Days of the period, 1 to 90.
        #[arg(long)]
        days: Option<i64>,
    },
    /// Account health (#719), when the app sends `account`: per account
    /// its users, active users and their share, weekly return, churned
    /// users, plan and state (healthy, at_risk, inactive), at risk first.
    /// Accounts with fewer than 3 users are counted together.
    Accounts {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Only this state: healthy, at_risk or inactive.
        #[arg(long)]
        state: Option<String>,
        /// Accounts listed, 1 to 500 (default 50).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// How sessions move between features (Sankey data, last 30 days).
    Flows {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Start at this feature.
        #[arg(long)]
        from: Option<String>,
        /// Only sessions that reached a success event.
        #[arg(long)]
        to_success: bool,
    },
    /// Comments pinned to moments of a replay: list them, or add one with
    /// --at and --add.
    Comments {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Session id.
        session: String,
        /// Seconds into the recording (with --add).
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        /// Add this comment.
        #[arg(long)]
        add: Option<String>,
    },
    /// Scoped API tokens: `read` for an MCP client, `write` for an agent
    /// that wires one app, `admin` for the install. Needs the admin token.
    Tokens {
        #[command(subcommand)]
        action: TokensCmd,
    },
    /// The environments of a project: host rules, aliases, hidden
    /// environments and per-environment checks (D-034).
    Env {
        #[command(subcommand)]
        action: EnvCmd,
    },
    /// The team: list, update, disable, enable or remove users (invite
    /// with `login-link --email`).
    Users {
        #[command(subcommand)]
        action: UsersCmd,
    },
    /// Upload the source maps of a build, so error stacks show original
    /// files and lines. Run it in the deploy step, with the same release
    /// the SDK sends.
    Sourcemaps {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Release of this build (as in `init({ release })` / data-release).
        #[arg(long)]
        release: String,
        /// Build output directory; every `*.map` file under it is uploaded.
        dir: PathBuf,
    },
    /// Record a release from the deploy step, after the deploy: its
    /// commit range (from `--from`, else the environment's previous
    /// deployed commit, to `--commit`), its source maps with
    /// `--sourcemaps`, and with `--wait` its verdict (polled for 30
    /// minutes). The release shows on the Releases page like a succeeded
    /// deploy.
    Release {
        /// Release name, the same the SDK sends.
        name: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// The commit deployed (7 to 64 hex digits).
        #[arg(long)]
        commit: Option<String>,
        /// The commit the range starts after.
        #[arg(long)]
        from: Option<String>,
        /// Environment deployed to.
        #[arg(long, default_value = "production")]
        env: String,
        /// Build output directory; every `*.map` file under it is
        /// uploaded for this release.
        #[arg(long)]
        sourcemaps: Option<PathBuf>,
        /// Wait for the release's verdict; exits non-zero when it is
        /// regressed or does not come in time.
        #[arg(long)]
        wait: bool,
    },
    /// Print the block to add to the app's AGENTS.md / CLAUDE.md so its
    /// agents keep the feature map current and every release measured.
    Rules,
    /// Create a one-time login link for the PM (valid 24 hours; invites: 7 days).
    LoginLink {
        /// Invite this person (else: an owner link, full access).
        #[arg(long)]
        email: Option<String>,
        /// Their role: viewer (look) or admin (change flags, invite).
        #[arg(long, default_value = "viewer")]
        role: String,
        /// Comma-separated project ids they may see (default: all).
        #[arg(long)]
        projects: Option<String>,
    },
    /// Run read-only SQL on a project's `events` table.
    Sql {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// The query.
        query: String,
    },
    /// Print the agent wiring guide served by the server.
    Guide,
    /// Run the MCP server for agents. Both roots are required: the server
    /// refuses any file access outside them.
    Mcp {
        /// Directory agents may read from.
        #[arg(long, required = true)]
        automation_read_root: PathBuf,
        /// Directory agents may write to.
        #[arg(long, required = true)]
        automation_write_root: PathBuf,
    },
}

#[derive(Subcommand)]
pub(super) enum SurveysCmd {
    /// List the project's surveys and their results.
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Create or update a survey.
    Set {
        /// Survey key.
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// The question (at most 200 characters).
        #[arg(long)]
        question: String,
        /// rating (1-5), yes_no, choice or text.
        #[arg(long)]
        kind: String,
        /// Options for kind=choice (comma-separated, 2-6).
        #[arg(long, value_delimiter = ',')]
        choices: Vec<String>,
        /// Event after which it is shown, e.g. entry_saved.
        #[arg(long)]
        trigger: String,
        /// Share of eligible users asked, 0-100.
        #[arg(long, default_value_t = 10)]
        sample: u8,
        /// Only these canonical roles (comma-separated).
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
        /// Create or leave it switched off.
        #[arg(long)]
        off: bool,
    },
    /// Delete a survey.
    Delete {
        /// Survey key.
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

#[derive(Subcommand)]
pub(super) enum FlagsCmd {
    /// List the project's flags.
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Create or update a flag.
    Set {
        /// Flag key, as used in code: openlens.isEnabled("<key>").
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Share of eligible users, 0-100.
        #[arg(long, default_value_t = 0)]
        rollout: u8,
        /// Create or leave it switched off.
        #[arg(long)]
        off: bool,
        /// Only these canonical roles (comma-separated).
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
        /// Always on for these accounts (comma-separated).
        #[arg(long, value_delimiter = ',')]
        accounts: Vec<String>,
        /// What the flag is for.
        #[arg(long, default_value = "")]
        description: String,
        /// A property rule, repeatable, all must match: `plan in pro,team`,
        /// `country not in FR`. A user who did not send the property is out.
        #[arg(long = "rule")]
        rules: Vec<String>,
    },
    /// Delete a flag.
    Delete {
        /// Flag key.
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

#[derive(Subcommand)]
pub(super) enum FeaturesCmd {
    /// Validate the map offline and check it against the app's code.
    Lint {
        /// Path to the map.
        #[arg(long, default_value = "openlens.features.yaml")]
        file: PathBuf,
        /// App source root to scan for data-feature attributes and routes.
        #[arg(long, default_value = ".")]
        src: PathBuf,
        /// Also measure the pushed map on real usage (score, findings
        /// with fixes, areas, adoption ladder): needs the server.
        #[arg(long)]
        live: bool,
        /// Project id (with --live).
        #[arg(long, default_value = "default")]
        project: String,
        /// With --live: fail (exit 1) below this score (0-100).
        #[arg(long)]
        min_score: Option<u8>,
    },
    /// Validate and upload the map (new version on the server).
    Push {
        /// Path to the map.
        #[arg(long, default_value = "openlens.features.yaml")]
        file: PathBuf,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Why this version (kept in the map's history).
        #[arg(long, short)]
        message: Option<String>,
    },
    /// One feature in depth: definition, health, numbers by role, top
    /// pages, adoption ladder (JSON).
    Context {
        /// Feature key (a former key works too).
        key: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}
