//! MCP profiles (#716, D-115): which tools `openlens-cli mcp` serves.
//!
//! The product profile (the default) holds the method's tools: wiring,
//! the feature map, the KPIs, releases and their verdict. The operations
//! profile holds running the app: uptime, the status page,
//! announcements, maintenance, job check-ins, availability and capacity.
//! `all` serves both. Every tool is in exactly one of the two lists below,
//! which the parity test (`mcp_parity.rs`) checks; a tool in neither is
//! served by no profile.

/// The set of tools one MCP server serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Profile {
    /// The product tools: wiring, map, KPIs, releases and their verdict.
    Product,
    /// The operations tools: uptime, status page, announcements,
    /// maintenance, jobs, availability, capacity.
    Ops,
    /// Every tool.
    All,
}

/// The product profile's tools. The release verdict and its inputs
/// (`release_report`, `releases`, `routes`, `runtime`, `delivery`,
/// `record_release`, `deploy_hook`) are the method's Control step.
pub(crate) const PRODUCT_TOOLS: &[&str] = &[
    "acquisition",
    "audience",
    "board",
    "board_job",
    "churn",
    "create_ticket",
    "delete_goal",
    "delete_objective",
    "delivery",
    "deploy_hook",
    "digest",
    "environments",
    "errors",
    "feature_context",
    "feature_map_lint",
    "feature_map_push",
    "feature_relations",
    "flag_set",
    "flag_update",
    "flags",
    "flows",
    "forms",
    "goals",
    "guide",
    "init",
    "insights",
    "kpi_report",
    "live",
    "login_link",
    "map_health",
    "map_overview",
    "next_actions",
    "projects",
    "propose_action",
    "record_release",
    "release_report",
    "releases",
    "routes",
    "runtime",
    "set_feature",
    "set_goal",
    "set_objective",
    "set_timezone",
    "sql",
    "stale_flags",
    "suggest_map_changes",
    "suggested_features",
    "survey_set",
    "surveys",
    "ticket_target",
    "tickets",
    "user_timeline",
    "verify",
    "vitals",
    "winback_flag",
];

/// The operations profile's tools. Their scope is frozen (fixes only)
/// until the product batches land (D-115).
pub(crate) const OPS_TOOLS: &[&str] = &[
    "alert_set",
    "alerts",
    "announce",
    "announcement_close",
    "announcement_delete",
    "announcement_edit",
    "announcement_send",
    "announcement_update",
    "announcements",
    "availability",
    "availability_target",
    "capacity",
    "check_in",
    "incidents",
    "job_set",
    "jobs",
    "maintenance",
    "maintenance_set",
    "status",
    "status_page",
    "status_page_set",
    "uptime_days",
];

impl Profile {
    /// The name given to `--profile`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Ops => "ops",
            Self::All => "all",
        }
    }

    /// Whether this profile serves `tool`.
    pub(crate) fn serves(self, tool: &str) -> bool {
        match self {
            Self::Product => PRODUCT_TOOLS.contains(&tool),
            Self::Ops => OPS_TOOLS.contains(&tool),
            Self::All => PRODUCT_TOOLS.contains(&tool) || OPS_TOOLS.contains(&tool),
        }
    }

    /// Why this profile refuses `tool`, naming the profile that has it, or
    /// `None` when no profile has it (an unknown tool).
    pub(crate) fn refusal(self, tool: &str) -> Option<String> {
        let home = [Self::Product, Self::Ops]
            .into_iter()
            .find(|p| p.serves(tool))?;
        Some(format!(
            "`{tool}` is in the {home} profile, and this server runs the {this} profile: \
             start `openlens-cli mcp --profile {home}` (or `--profile all`) to use it",
            home = home.name(),
            this = self.name(),
        ))
    }
}
