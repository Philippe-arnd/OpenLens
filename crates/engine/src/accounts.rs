//! Account health (#719, D-117), when the app sends `account`
//! (`identify(id, { account })`): per account over the last 30 days, its
//! known users, the active ones and their share, the weekly return of its
//! users, the users it lost, its plan, and a state (healthy, at risk,
//! inactive). Accounts with fewer than 3 known users are counted together
//! as small accounts, never named. For an internal tool an account reads
//! as a team; the code says account. Definitions in `docs/kpis.md`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use openlens_query::QueryEngine;
use serde::{Deserialize, Serialize};

use crate::{Filters, INTERACTION, NOT_SERVER, Result, before, int, text};

/// Days of the period; the 30 before it are the comparison.
pub const DAYS: i64 = 30;
/// Days without an active user after which an account is inactive.
pub const INACTIVE_DAYS: i64 = 14;
/// Known users an account needs to be listed by name; fewer are counted
/// with the small accounts (privacy and noise).
pub const MIN_USERS: i64 = 3;

/// An account's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    /// Its active share fell by a third or more against the 30 days
    /// before, or a quarter or more of its users churned.
    AtRisk,
    /// No active user for 14 days.
    Inactive,
    /// Neither.
    Healthy,
}

impl AccountState {
    /// The state a request names: `healthy`, `at_risk` (or `at risk`),
    /// `inactive`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "healthy" => Some(Self::Healthy),
            "at_risk" => Some(Self::AtRisk),
            "inactive" => Some(Self::Inactive),
            _ => None,
        }
    }

    /// In plain words.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AtRisk => "at risk",
            Self::Inactive => "inactive",
            Self::Healthy => "healthy",
        }
    }
}

/// One account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountHealth {
    /// The account as the app sends it.
    pub account: String,
    /// Its known users: identified with this account, the team's own left
    /// out, or seen with it in the last 60 days when that is more.
    pub users: i64,
    /// Of them, active (an interaction) in the last 30 days.
    pub active: i64,
    /// `active` over `users`.
    pub active_share: f64,
    /// The same share over the 30 days before.
    pub previous_share: f64,
    /// Of its users active 8 to 14 days ago, the share active again in the
    /// last 7 days; none when nobody was.
    pub retention: Option<f64>,
    /// Users active in the 30 days before and not in the last 30.
    pub churned: i64,
    /// `churned` over `users`.
    pub churned_share: f64,
    /// The plan most of its users have now; none when none has one.
    pub plan: Option<String>,
    /// Its last interaction (UTC day), within the 60 days read.
    pub last_active: Option<String>,
    /// Its state.
    pub state: AccountState,
}

/// The accounts with fewer than [`MIN_USERS`] known users, together.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SmallAccounts {
    /// How many.
    pub accounts: i64,
    /// Their known users.
    pub users: i64,
    /// Of them, active in the last 30 days.
    pub active: i64,
}

/// Account health of a project.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Accounts {
    /// Environment read.
    pub environment: String,
    /// Days of the period.
    pub days: i64,
    /// The accounts, at risk first, then inactive, then healthy (after
    /// the state filter and the limit).
    pub accounts: Vec<AccountHealth>,
    /// Listed accounts at risk (before the filter and the limit).
    pub at_risk: usize,
    /// Listed accounts inactive.
    pub inactive: usize,
    /// Listed accounts healthy.
    pub healthy: usize,
    /// The small accounts, counted together.
    pub small: SmallAccounts,
    /// More accounts than one read returns: the list is partial.
    pub truncated: bool,
    /// Why the list is empty, when it is.
    pub note: Option<String>,
}

/// What the events say about one account over the last 60 days.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Activity {
    /// Its people seen in the 60 days.
    pub seen: i64,
    /// Active in the last 30 days.
    pub active: i64,
    /// Active in the 30 days before.
    pub before: i64,
    /// Active before and not since.
    pub churned: i64,
    /// Active 8 to 14 days ago.
    pub week_before: i64,
    /// Of them, active again in the last 7 days.
    pub returned: i64,
    /// Active in the last [`INACTIVE_DAYS`].
    pub recent: i64,
    /// Last interaction (UTC day).
    pub last: Option<String>,
}

/// The state of an account from its numbers.
#[must_use]
pub fn state(users: i64, a: &Activity) -> AccountState {
    if a.recent == 0 {
        return AccountState::Inactive;
    }
    // A third or more down, on the same users: active ≤ ⅔ of before.
    let fell = a.before > 0 && 3 * a.active <= 2 * a.before;
    let lost = a.churned > 0 && 4 * a.churned >= users.max(1);
    if fell || lost {
        AccountState::AtRisk
    } else {
        AccountState::Healthy
    }
}

/// An account's known users: the total, and how many per plan.
type KnownUsers<'a> = (i64, Vec<(Option<&'a str>, i64)>);

/// The accounts from the known users per plan (`(account, plan, users)`,
/// from the people table) and the activity per account; at risk first.
#[must_use]
pub fn assemble(
    known: &[(String, Option<String>, i64)],
    activity: &BTreeMap<String, Activity>,
) -> (Vec<AccountHealth>, SmallAccounts) {
    let mut people: BTreeMap<&str, KnownUsers<'_>> = BTreeMap::new();
    for (account, plan, n) in known {
        let e = people.entry(account.as_str()).or_default();
        e.0 += n;
        e.1.push((plan.as_deref(), *n));
    }
    for account in activity.keys() {
        people.entry(account.as_str()).or_default();
    }
    let none = Activity::default();
    let mut out = Vec::new();
    let mut small = SmallAccounts::default();
    for (account, (known_users, plans)) in people {
        let a = activity.get(account).unwrap_or(&none);
        let users = known_users.max(a.seen);
        if users <= 0 {
            continue;
        }
        let active = a.active.min(users);
        if users < MIN_USERS {
            small.accounts += 1;
            small.users += users;
            small.active += active;
            continue;
        }
        // The plan most of its users have; ties go to the first by name.
        let plan = plans
            .iter()
            .filter_map(|(p, n)| p.map(|p| (p, *n)))
            .fold(None::<(&str, i64)>, |best, (p, n)| match best {
                Some((bp, bn)) if bn > n || (bn == n && bp <= p) => Some((bp, bn)),
                _ => Some((p, n)),
            })
            .map(|(p, _)| p.to_owned());
        let share = |n: i64| n.min(users) as f64 / users as f64;
        out.push(AccountHealth {
            account: account.to_owned(),
            users,
            active,
            active_share: share(a.active),
            previous_share: share(a.before),
            retention: (a.week_before > 0).then(|| a.returned as f64 / a.week_before as f64),
            churned: a.churned.min(users),
            churned_share: share(a.churned),
            plan,
            last_active: a.last.clone(),
            state: state(users, a),
        });
    }
    out.sort_by(|x, y| {
        x.state
            .cmp(&y.state)
            .then_with(|| {
                let drop = |h: &AccountHealth| h.previous_share - h.active_share;
                match x.state {
                    AccountState::AtRisk => drop(y).total_cmp(&drop(x)),
                    AccountState::Inactive => y.users.cmp(&x.users),
                    AccountState::Healthy => x.active_share.total_cmp(&y.active_share),
                }
            })
            .then_with(|| y.users.cmp(&x.users))
            .then_with(|| x.account.cmp(&y.account))
    });
    (out, small)
}

/// The activity per account over the last 60 days in `f`'s environment
/// (interactions of identified or anonymous people with an `account`, the
/// team's own left out), and whether the read was cut.
pub async fn activity(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    now: DateTime<Utc>,
) -> Result<(BTreeMap<String, Activity>, bool)> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause_at(2 * DAYS, now)?;
    let day = 86_400;
    let (t30, t14, t7, t_recent) = (
        before(now, DAYS * day),
        before(now, 14 * day),
        before(now, 7 * day),
        before(now, INACTIVE_DAYS * day),
    );
    let sql = format!(
        "SELECT account, count(*) AS seen, sum(a) AS active, sum(b) AS before_n, \
                sum(CASE WHEN b = 1 AND a = 0 THEN 1 ELSE 0 END) AS churned, \
                sum(w2) AS week_before, sum(CASE WHEN w2 = 1 AND w1 = 1 THEN 1 ELSE 0 END) AS returned, \
                sum(r) AS recent, CAST(max(last_ts) AS VARCHAR) AS last \
         FROM (SELECT account, distinct_id, \
                      max(CASE WHEN timestamp >= {t30} THEN 1 ELSE 0 END) AS a, \
                      max(CASE WHEN timestamp < {t30} THEN 1 ELSE 0 END) AS b, \
                      max(CASE WHEN timestamp >= {t7} THEN 1 ELSE 0 END) AS w1, \
                      max(CASE WHEN timestamp >= {t14} AND timestamp < {t7} THEN 1 ELSE 0 END) AS w2, \
                      max(CASE WHEN timestamp >= {t_recent} THEN 1 ELSE 0 END) AS r, \
                      max(timestamp) AS last_ts \
               FROM events WHERE {w} AND account IS NOT NULL AND account <> '' \
               AND {NOT_SERVER} AND {INTERACTION} \
               GROUP BY account, distinct_id) u \
         GROUP BY account"
    );
    let r = q.sql(project, &sql).await?;
    let map = r
        .rows
        .iter()
        .filter_map(|row| {
            let account = text(row, "account")?;
            Some((
                account,
                Activity {
                    seen: int(row, "seen"),
                    active: int(row, "active"),
                    before: int(row, "before_n"),
                    churned: int(row, "churned"),
                    week_before: int(row, "week_before"),
                    returned: int(row, "returned"),
                    recent: int(row, "recent"),
                    last: text(row, "last").map(|t| t.chars().take(10).collect()),
                },
            ))
        })
        .collect();
    Ok((map, r.truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(users: i64, before: i64, now: i64) -> Activity {
        Activity {
            seen: users,
            active: now,
            before,
            churned: (before - now).max(0),
            week_before: now,
            returned: now,
            recent: now,
            last: Some("2026-11-27".into()),
        }
    }

    /// An account whose active users fall by half is at risk and leads
    /// the list; a steady one is healthy; one quiet for 14 days is
    /// inactive; a 2-user account is only counted with the small ones.
    #[test]
    fn a_halved_account_is_at_risk_and_small_ones_are_grouped() {
        let known = vec![
            ("acme".to_owned(), Some("pro".to_owned()), 3),
            ("acme".to_owned(), Some("team".to_owned()), 1),
            ("steady".to_owned(), Some("team".to_owned()), 4),
            ("quiet".to_owned(), None, 4),
            ("duo".to_owned(), Some("pro".to_owned()), 2),
        ];
        let mut activity = BTreeMap::new();
        activity.insert("acme".to_owned(), active(4, 4, 2));
        activity.insert("steady".to_owned(), active(4, 4, 4));
        activity.insert(
            "quiet".to_owned(),
            Activity {
                recent: 0,
                ..active(4, 4, 3)
            },
        );
        activity.insert("duo".to_owned(), active(2, 2, 2));
        let (list, small) = assemble(&known, &activity);
        let names: Vec<&str> = list.iter().map(|a| a.account.as_str()).collect();
        assert_eq!(names, ["acme", "quiet", "steady"]);
        let acme = &list[0];
        assert_eq!(acme.state, AccountState::AtRisk);
        assert!(acme.active_share < acme.previous_share);
        assert_eq!(acme.plan.as_deref(), Some("pro"));
        assert_eq!(list[1].state, AccountState::Inactive);
        assert_eq!(list[2].state, AccountState::Healthy);
        assert!(!names.contains(&"duo"));
        assert_eq!(small.accounts, 1);
        assert_eq!(small.users, 2);
        // A drop of less than a third, with nobody lost, stays healthy.
        let mild = Activity {
            churned: 0,
            ..active(9, 9, 7)
        };
        assert_eq!(state(9, &mild), AccountState::Healthy);
    }

    #[test]
    fn no_account_sent_lists_nothing() {
        let (list, small) = assemble(&[], &BTreeMap::new());
        assert!(list.is_empty());
        assert_eq!(small, SmallAccounts::default());
    }
}
