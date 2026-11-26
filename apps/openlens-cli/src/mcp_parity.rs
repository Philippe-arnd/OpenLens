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
    ("tickets.list", "tickets"),
    ("tickets.draft", "create_ticket"),
    ("tickets.file", "create_ticket"),
    ("deploys.rotate_hook", "deploy_hook"),
    ("releases.record", "record_release"),
];

/// Commands with no MCP tool yet. Cut this list down by adding the tool.
const PENDING: &[&str] = &[
    "goals.delete",
    "goals.set_objective",
    "goals.delete_objective",
    "alerts.status",
    "alerts.uptime",
    "alerts.settings",
    "alerts.set",
    "alerts.test",
    "tickets.target",
    // Run by the deploy hook (`/hooks/deploy/<secret>`), which a platform
    // calls, not an agent.
    "deploys.record",
    // Ask's lookups and the flags commands (#508), explore (#507): the
    // MCP tools still call the REST routes these commands back.
    "ask.board",
    "ask.feature",
    "ask.errors",
    "ask.vitals",
    "ask.goals",
    "ask.sql",
    "flags.list",
    "flags.update",
    "explore.person",
    "explore.breakdown",
    "explore.sql",
    "explore.acquisition",
    "explore.forms",
    "explore.churn",
    "explore.flag_audiences",
    "explore.make_audience",
    "explore.vitals",
    "explore.audience",
    "explore.flows",
    "explore.live",
    "board.estimate",
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
    "kpi_report",
    "user_timeline",
    "board",
    "vitals",
    "errors",
    "sql",
    "flags",
    "flag_set",
    "surveys",
    "survey_set",
    "login_link",
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
