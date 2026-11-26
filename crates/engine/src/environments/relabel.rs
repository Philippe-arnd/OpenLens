//! Re-labelling past events after a host rule changed (#484).

use openlens_event::sdk_guess;
use serde::Serialize;

use super::{BOT_ENVIRONMENT, Resolution, Rules, by_host, check_pattern, with_alias};

/// Days a re-labelling reaches back by default (#484).
pub const RELABEL_DAYS: i64 = 30;
/// Days a re-labelling reaches back at most: the rewrite reads every file
/// of those days, so it stays bounded.
pub const MAX_RELABEL_DAYS: i64 = 90;

/// The host pattern (exact or `*.suffix`, as rules take it) and the days
/// of a re-labelling, checked: 1 to [`MAX_RELABEL_DAYS`], default
/// [`RELABEL_DAYS`].
pub fn check_relabel(
    host: &str,
    days: Option<i64>,
) -> std::result::Result<(String, i64), &'static str> {
    let pattern = check_pattern(host)?;
    let days = days.unwrap_or(RELABEL_DAYS);
    if !(1..=MAX_RELABEL_DAYS).contains(&days) {
        return Err("days must be between 1 and 90");
    }
    Ok((pattern, days))
}

/// Whether `host` is `pattern`, or under it when it is `*.suffix`.
#[must_use]
pub fn covers(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|under| under.ends_with('.')),
        None => host == pattern,
    }
}

/// Where a past event of `host`, stored as `stored`, goes when re-labelled
/// (engine command `relabel_environment`, #484): the environment a newly
/// detected event of that host gets from the current rules and aliases.
/// `None` when it stays: already there, a bot, or a host the rules now
/// `ignore` (re-labelling never deletes events). What the app set itself
/// is not told apart once stored, so it moves too; the dry run shows each
/// move first.
#[must_use]
pub fn relabel_target(rules: &Rules, seeded: bool, host: &str, stored: &str) -> Option<String> {
    if stored == BOT_ENVIRONMENT {
        return None;
    }
    let guess = sdk_guess(host);
    let to = match with_alias(rules, by_host(rules, seeded, host, guess), guess) {
        Resolution::Keep => guess.to_owned(),
        Resolution::Set(name) => name,
        Resolution::Drop => return None,
    };
    (to != stored).then_some(to)
}

/// One move of a re-labelling: how many events go from one environment
/// to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelabelMove {
    /// The environment they are stored under.
    pub from: String,
    /// The environment the rules give now.
    pub to: String,
    /// Events.
    pub events: usize,
}

/// What a re-labelling moved, or would move on a dry run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relabelled {
    /// The host pattern.
    pub host: String,
    /// Days read, today (UTC) included.
    pub days: i64,
    /// Nothing was written.
    pub dry_run: bool,
    /// Events moved (or that would move).
    pub events: usize,
    /// Per (stored environment, new environment), most events first.
    pub moves: Vec<RelabelMove>,
}

impl Relabelled {
    /// The result of a re-labelling from its moves.
    #[must_use]
    pub fn new(
        host: String,
        days: i64,
        dry_run: bool,
        moves: impl IntoIterator<Item = ((String, String), usize)>,
    ) -> Self {
        let mut moves: Vec<RelabelMove> = moves
            .into_iter()
            .map(|((from, to), events)| RelabelMove { from, to, events })
            .collect();
        moves.sort_by_key(|m| std::cmp::Reverse(m.events));
        Self {
            host,
            days,
            dry_run,
            events: moves.iter().map(|m| m.events).sum(),
            moves,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{set_environment_alias, set_environment_rule};
    use super::*;

    fn rules(list: &[(&str, &str)]) -> Rules {
        let mut r = Rules::default();
        for (h, e) in list {
            assert!(set_environment_rule(&mut r, h, e).is_ok(), "{h} → {e}");
        }
        r
    }

    #[test]
    fn relabelling_gives_what_a_new_event_of_the_host_would_get() {
        let mut r = rules(&[
            ("kanban-test.example.com", "qa"),
            ("*.preview.example.com", "preview"),
        ]);
        let h = "kanban-test.example.com";
        // An older rule had made it staging: the current rule wins.
        assert_eq!(
            relabel_target(&r, true, h, "staging").as_deref(),
            Some("qa")
        );
        assert_eq!(relabel_target(&r, true, h, "qa"), None, "already there");
        assert_eq!(relabel_target(&r, true, h, "bot"), None, "bots stay bots");
        // A host with no rule and no pattern: unknown once seeded.
        assert_eq!(
            relabel_target(&r, true, "other.example.com", "production").as_deref(),
            Some("unknown")
        );
        assert_eq!(
            relabel_target(&r, false, "other.example.com", "production"),
            None
        );
        // Aliases apply on top, and `ignore` never deletes.
        assert!(set_environment_alias(&mut r, "qa", "staging").is_ok());
        assert_eq!(
            relabel_target(&r, true, h, "qa").as_deref(),
            Some("staging")
        );
        assert!(set_environment_rule(&mut r, h, "ignore").is_ok());
        assert_eq!(relabel_target(&r, true, h, "qa"), None);

        assert!(covers("*.preview.example.com", "pr-1.preview.example.com"));
        assert!(!covers("*.preview.example.com", "preview.example.com"));
        assert!(!covers("*.example.com", "badexample.com"));
        assert!(covers("app.example.com", "app.example.com"));
        assert!(check_relabel("app.example.com", Some(91)).is_err());
        assert!(check_relabel("app.example.com", Some(0)).is_err());
        assert_eq!(
            check_relabel(" App.Example.com ", None),
            Ok(("app.example.com".into(), 30))
        );
    }
}
