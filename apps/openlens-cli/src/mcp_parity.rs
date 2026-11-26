//! MCP parity (#509, D-058): every engine command is an MCP tool, or is
//! listed below as not one yet. A new command without a tool, a new tool
//! without a command, or a stale entry in these lists fails the test, so
//! "anything you can do in the UI, an agent can do through MCP" (rule 10)
//! is checked rather than hoped for.

use openlens_engine::commands::REGISTRY;
use openlens_mcp::Tools;

use crate::Client;
use crate::mcp::OpenLensTools;

/// The tool that runs each command (names differ: commands are
/// `area.verb`, tools are what an agent asks for). One tool may run
/// several commands.
const TOOL_OF: &[(&str, &str)] = &[
    ("goals.list", "goals"),
    ("goals.set", "set_goal"),
    ("releases.list", "releases"),
    ("releases.report", "release_report"),
    ("releases.delivery", "delivery"),
    ("tickets.list", "tickets"),
    ("tickets.draft", "create_ticket"),
    ("tickets.file", "create_ticket"),
    ("deploys.rotate_hook", "deploy_hook"),
    // #525: the tools of `mcp_commands.rs`, and the existing tools whose
    // REST route runs the command.
    ("goals.delete", "delete_goal"),
    ("goals.set_objective", "set_objective"),
    ("goals.delete_objective", "delete_objective"),
    ("alerts.status", "status"),
    ("alerts.uptime", "alerts"),
    ("alerts.settings", "alerts"),
    ("alerts.set", "alert_set"),
    ("alerts.test", "alert_set"),
    ("tickets.target", "ticket_target"),
    ("flags.list", "flags"),
    ("flags.update", "flag_update"),
    ("explore.person", "user_timeline"),
    ("explore.breakdown", "kpi_report"),
    ("explore.acquisition", "acquisition"),
    ("explore.forms", "forms"),
    ("explore.churn", "churn"),
    ("explore.flag_audiences", "churn"),
    ("explore.make_audience", "winback_flag"),
    ("explore.vitals", "vitals"),
    ("explore.audience", "audience"),
    ("explore.flows", "flows"),
    ("explore.live", "live"),
    // `GET /api/projects/{id}/board` runs it for a date range or a past
    // release before reading the board.
    ("board.estimate", "board"),
    ("releases.record", "record_release"),
];

/// Commands deliberately without an MCP tool, each with its reason.
const PENDING: &[&str] = &[
    // Run by the deploy hook (`/hooks/deploy/<secret>`), which a platform
    // calls, not an agent.
    "deploys.record",
    // Ask's own lookups, in Ask's fixed filters (D-057). Agents get the same
    // data from `board`/`insights`, `feature_context`, `errors`, `vitals`,
    // `goals` and `sql`, whose routes run other code.
    "ask.board",
    "ask.feature",
    "ask.errors",
    "ask.vitals",
    "ask.goals",
    "ask.sql",
    // The SQL page's query. The `sql` tool runs the same query through
    // `POST /api/projects/{id}/sql`, which answers a busy engine with a 503
    // and `Retry-After` where the command only has an error text.
    "explore.sql",
];

/// Tools that are not engine commands yet: their logic still sits behind
/// the REST API (or in the CLI, for the files an agent reads). `sql` is
/// the raw read-only SQL, an escape hatch rather than an action.
const NOT_COMMANDS: &[&str] = &[
    "guide",
    "projects",
    "init",
    "set_timezone",
    "feature_map_lint",
    "feature_map_push",
    "map_health",
    "map_overview",
    "feature_context",
    "feature_relations",
    "suggest_map_changes",
    "environments",
    "verify",
    "insights",
    "digest",
    "board_job",
    "errors",
    "sql",
    "flag_set",
    "surveys",
    "survey_set",
    "login_link",
    // The agent kit (#524, D-069): map health's findings as entries, one
    // entry pushed through the feature-map route, flags judged from the
    // flags route and SQL.
    "suggested_features",
    "set_feature",
    "stale_flags",
];

#[test]
fn every_command_is_a_tool_or_pending_and_every_tool_is_accounted_for() {
    let commands: Vec<&str> = REGISTRY.iter().map(|s| s.name).collect();
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(Some("http://localhost:1".into()), Some("t".into())).unwrap();
    let tools = OpenLensTools::new(client, dir.path(), dir.path()).unwrap();
    let tools: Vec<&str> = tools.list().iter().map(|t| t.name).collect();
    for c in &commands {
        let mapped = TOOL_OF.iter().any(|(m, _)| m == c);
        let pending = PENDING.contains(c);
        assert!(
            mapped != pending,
            "command `{c}`: give it an MCP tool and add it to TOOL_OF, or list it in PENDING (not both)"
        );
    }
    for (c, t) in TOOL_OF {
        assert!(
            commands.contains(c),
            "TOOL_OF names `{c}`, which is not a command"
        );
        assert!(
            tools.contains(t),
            "TOOL_OF maps `{c}` to `{t}`, which is not a tool"
        );
    }
    for c in PENDING {
        assert!(
            commands.contains(c),
            "PENDING names `{c}`, which is not a command"
        );
    }
    for t in &tools {
        let runs = TOOL_OF.iter().any(|(_, m)| m == t);
        let not_command = NOT_COMMANDS.contains(t);
        assert!(
            runs != not_command,
            "tool `{t}`: map the command it runs in TOOL_OF, or list it in NOT_COMMANDS (not both)"
        );
    }
    for t in NOT_COMMANDS {
        assert!(
            tools.contains(t),
            "NOT_COMMANDS names `{t}`, which is not a tool"
        );
    }
}
