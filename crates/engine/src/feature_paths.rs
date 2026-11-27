//! What left the board for the pages that own it (#620): one feature's
//! declared funnel and its paths to its success event, for its page, and
//! the full roles × features matrix, for the Feature map. Both read the
//! same rows as the board's blocks did (people rows, `session_feature_day`
//! and `success_day` for the closed days rolled with the map in force, the
//! events for the rest), over the board's fixed 30 days.

use super::*;

/// One feature's funnel and the paths that lead to its success event.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeaturePaths {
    /// Its declared steps (the success event appended when it is not the
    /// last one), when it declares 2 to 8; `None` otherwise.
    pub funnel: Option<Funnel>,
    /// The 3 most frequent paths into its first success, when it declares
    /// a success event (paths of fewer than 3 users are left out).
    pub journeys: Vec<Journey>,
}

/// Users per role and per role × feature, last 30 days, every role.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RoleMatrix {
    /// Users per canonical role, most first (12 at most).
    pub roles: Vec<RoleUsers>,
    /// (role, feature, users).
    pub cells: Vec<(String, String, i64)>,
    /// Raw role values the map does not name, with their users.
    pub gaps: Vec<(String, i64)>,
}

/// The stored version that `m` is, when it is one (its closed days are
/// read from the rollups made with it).
fn version_of(m: &FeatureMap, history: &crate::inforce::History) -> Option<i64> {
    history
        .versions
        .iter()
        .filter(|h| h.map == *m)
        .map(|h| h.version)
        .max()
}

/// [`FeaturePaths`] for the feature `key` of `m` (empty when the map does
/// not declare it).
pub async fn feature_paths(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    history: &crate::inforce::History,
    f: &Filters,
    key: &str,
) -> Result<FeaturePaths> {
    let Some(def) = m.features.iter().find(|x| x.key == key) else {
        return Ok(FeaturePaths::default());
    };
    let q = &q.with_params(&f.params()?);
    let src = source(Some(m));
    let mut steps = def.steps.clone();
    if !steps.is_empty()
        && let Some(s) = &def.success
        && steps.last() != Some(s)
    {
        steps.push(s.clone());
    }
    let defs = if steps.is_empty() {
        Vec::new()
    } else {
        vec![(def.key.clone(), steps)]
    };
    let successes: Vec<(String, String)> = def
        .success
        .iter()
        .map(|s| (def.key.clone(), s.clone()))
        .collect();
    let journeys = async {
        if successes.is_empty() {
            return Ok(Vec::new());
        }
        let version = version_of(m, history);
        let now = Utc::now();
        let (features, wins) = futures::try_join!(
            board_people::rolled_days(q, project, "session_feature_day", version, now),
            board_people::rolled_days(q, project, "success_day", version, now),
        )?;
        let role = lookups(m).0;
        let sf = board_sessions::feature_rows(&src, &role, &features, now, f, 30)?;
        let firsts = board_success::SuccessRows {
            src: &src,
            role,
            rolled: wins,
            now,
        };
        insights::journeys(q, project, &firsts, &sf, f, &successes).await
    };
    let (funnels, journeys) =
        futures::try_join!(insights::funnels(q, project, &src, f, &defs), journeys)?;
    Ok(FeaturePaths {
        funnel: funnels.into_iter().next(),
        journeys,
    })
}

/// [`RoleMatrix`] for the map `m` (`version`: the stored version it is,
/// when it is one), for the filters `f` with any role.
pub async fn role_matrix(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    version: Option<i64>,
    f: &Filters,
) -> Result<RoleMatrix> {
    let any_role = Filters {
        role: None,
        ..f.clone()
    };
    let q = &q.with_params(&any_role.params()?);
    let src = source(Some(m));
    let now = Utc::now();
    let rolled = board_people::rolled_days(q, project, "person_day", version, now).await?;
    let people = board_people::People {
        src: &src,
        role: lookups(m).0,
        task: "feature IS NOT NULL",
        rolled,
        now,
    };
    let rows = people.rows(&any_role, 30)?;
    let roles_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {rows} \
         GROUP BY role ORDER BY users DESC LIMIT 12"
    );
    let cells_sql = format!(
        "SELECT role, feature, count(DISTINCT distinct_id) AS users FROM {rows} \
         WHERE role IS NOT NULL AND feature IS NOT NULL GROUP BY role, feature"
    );
    let (roles, cells) =
        futures::try_join!(q.sql(project, &roles_sql), q.sql(project, &cells_sql))?;
    let mut out = RoleMatrix::default();
    for r in &roles.rows {
        let role = text(r, "role");
        let users = int(r, "users");
        match role.as_deref().and_then(|v| v.strip_prefix("unmatched:")) {
            Some(raw) => out.gaps.push((raw.to_owned(), users)),
            None => out.roles.push(RoleUsers { role, users }),
        }
    }
    out.cells = cells
        .rows
        .iter()
        .filter_map(|r| Some((text(r, "role")?, text(r, "feature")?, int(r, "users"))))
        .collect();
    Ok(out)
}
