//! The people and data explorer as commands (#222, #507): a person's
//! profile, the breakdown, acquisition, forms, churn, audience, flows,
//! live and Web Vitals reports, read-only SQL for admins, and the win-back
//! flag made from the users at risk (#269). Moved out of the server as is;
//! the pages, exports and REST answers are adapters over these.

use openlens_featuremap::FeatureMap;
use openlens_meta::{Person, Project};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use super::{Access, Caller, Command, Error, Host};
use crate::forms::FormStats;
use crate::{
    AcquisitionRow, Audience, BreakdownRow, ChurnReport, EngineError, EventRow, Filters, Flow,
    FlowQuery, Live, Touch, VitalReport,
};

/// The query the SQL page opens with.
pub const DEFAULT_SQL: &str = "SELECT event, count(*) AS n FROM events\nWHERE date >= current_date - INTERVAL '7 days'\nGROUP BY event ORDER BY n DESC";

async fn project<H: Host>(host: &H, id: &str) -> Result<Project, Error> {
    host.project(id).await.ok_or(Error::NotFound)
}

async fn map<H: Host>(host: &H, project: &str) -> Option<FeatureMap> {
    host.feature_map(project).await.map(|(m, _)| m)
}

fn unavailable(e: EngineError) -> Error {
    Error::Unavailable(e.to_string())
}

/// A project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    pub project: String,
}

/// A project and the filters a report honours.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilteredInput {
    /// Project id.
    pub project: String,
    /// The board filters, with the time zone to count days in.
    pub filters: Filters,
}

/// The input of `explore.person`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersonInput {
    /// Project id.
    pub project: String,
    /// The app's user id.
    pub distinct_id: String,
}

/// A person's profile: what is known of them, their roles over time,
/// their recordings and their last 100 events.
pub struct Profile {
    /// As last identified, when they were.
    pub person: Option<Person>,
    /// (role, since), oldest first.
    pub roles: Vec<(String, String)>,
    /// Their session recordings.
    pub replays: Vec<openlens_replay::Summary>,
    /// Their latest events.
    pub events: Vec<EventRow>,
}

impl Serialize for Profile {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        json!({
            "person": self.person.as_ref().map(|p| json!({
                "distinct_id": p.distinct_id, "role": p.role, "account": p.account,
                "plan": p.plan, "first_seen": p.first_seen, "last_seen": p.last_seen,
            })),
            "roles": self.roles,
            "replays": self.replays,
            "events": self.events,
        })
        .serialize(s)
    }
}

/// `explore.person`: a person's profile.
pub struct GetPerson;

impl Command for GetPerson {
    const NAME: &'static str = "explore.person";
    const ACCESS: Access = Access::Read;
    type Input = PersonInput;
    type Output = Profile;

    async fn run<H: Host>(host: &H, _: &Caller, input: PersonInput) -> Result<Profile, Error> {
        let p = project(host, &input.project).await?;
        let who = input.distinct_id;
        if who.is_empty() || who.len() > 200 {
            return Err(Error::Invalid("No such person.".into()));
        }
        let map = map(host, &p.id).await;
        let person = host.meta().person(&p.id, &who).ok().flatten();
        let roles = host.meta().role_history(&p.id, &who).unwrap_or_default();
        let replays = openlens_replay::list(host.data_dir(), &p.id, Some(&who)).unwrap_or_default();
        let events = crate::person_events(host.query(), &p.id, map.as_ref(), &who, 100)
            .await
            .unwrap_or_default();
        if person.is_none() && events.is_empty() {
            return Err(Error::Invalid(
                "Nobody with this id in the last 90 days.".into(),
            ));
        }
        Ok(Profile {
            person,
            roles,
            replays,
            events,
        })
    }
}

/// The input of `explore.breakdown`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BreakdownInput {
    /// Project id.
    pub project: String,
    /// The board filters.
    pub filters: Filters,
    /// `role`, `plan`, `account`, `environment`, `release`, `page` or
    /// `feature`.
    pub by: String,
}

/// `explore.breakdown`: the KPIs per value of a dimension.
pub struct Breakdown;

impl Command for Breakdown {
    const NAME: &'static str = "explore.breakdown";
    const ACCESS: Access = Access::Read;
    type Input = BreakdownInput;
    type Output = Vec<BreakdownRow>;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: BreakdownInput,
    ) -> Result<Vec<BreakdownRow>, Error> {
        let p = project(host, &input.project).await?;
        let map = map(host, &p.id).await;
        crate::breakdown(host.query(), &p.id, map.as_ref(), &input.filters, &input.by)
            .await
            .map_err(unavailable)
    }
}

/// The input of `explore.sql`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SqlInput {
    /// Project id.
    pub project: String,
    /// One read-only statement.
    pub sql: String,
}

/// `explore.sql`: run a read-only query on the project's events (the
/// API's limits).
pub struct Sql;

impl Command for Sql {
    const NAME: &'static str = "explore.sql";
    const ACCESS: Access = Access::Write;
    type Input = SqlInput;
    type Output = openlens_query::QueryResult;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: SqlInput,
    ) -> Result<openlens_query::QueryResult, Error> {
        let p = project(host, &input.project).await?;
        host.query()
            .cold()
            .sql(&p.id, &input.sql)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))
    }
}

/// The input of `explore.acquisition`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcquisitionInput {
    /// Project id.
    pub project: String,
    /// The board filters.
    pub filters: Filters,
    /// `channel` (default), `source`, `campaign` or `referrer`.
    #[serde(default)]
    pub by: Option<String>,
    /// `first` (default) or `last`.
    #[serde(default)]
    pub touch: Option<String>,
}

/// The acquisition table, with what it is grouped by.
#[derive(Debug, Clone, Serialize)]
pub struct Acquisition {
    /// `channel`, `source`, `campaign` or `referrer`.
    pub by: &'static str,
    /// Grouped by the last touch rather than the first.
    pub last: bool,
    /// The rows.
    pub rows: Vec<AcquisitionRow>,
}

/// What an acquisition table is grouped by: `source`, `campaign` or
/// `referrer` when asked, `channel` otherwise.
#[must_use]
pub fn acquisition_by(by: Option<&str>) -> &'static str {
    ["source", "campaign", "referrer"]
        .into_iter()
        .find(|b| by == Some(*b))
        .unwrap_or("channel")
}

/// `explore.acquisition`: users per channel, source, campaign or referrer
/// (#226).
pub struct GetAcquisition;

impl Command for GetAcquisition {
    const NAME: &'static str = "explore.acquisition";
    const ACCESS: Access = Access::Read;
    type Input = AcquisitionInput;
    type Output = Acquisition;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: AcquisitionInput,
    ) -> Result<Acquisition, Error> {
        let p = project(host, &input.project).await?;
        let by = acquisition_by(input.by.as_deref());
        let last = input.touch.as_deref() == Some("last");
        let touch = if last { Touch::Last } else { Touch::First };
        let map = map(host, &p.id).await;
        let rows = crate::acquisition(host.query(), &p.id, map.as_ref(), &input.filters, by, touch)
            .await
            .map_err(unavailable)?;
        Ok(Acquisition { by, last, rows })
    }
}

/// `explore.forms`: the forms and where people give up (#232).
pub struct Forms;

impl Command for Forms {
    const NAME: &'static str = "explore.forms";
    const ACCESS: Access = Access::Read;
    type Input = FilteredInput;
    type Output = Vec<FormStats>;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: FilteredInput,
    ) -> Result<Vec<FormStats>, Error> {
        let p = project(host, &input.project).await?;
        let map = map(host, &p.id).await;
        crate::forms::forms(host.query(), &p.id, map.as_ref(), &input.filters)
            .await
            .map_err(unavailable)
    }
}

/// The input of `explore.churn`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChurnInput {
    /// Project id.
    pub project: String,
    /// The board filters; the defaults in the project's time zone when
    /// absent.
    #[serde(default)]
    pub filters: Option<Filters>,
}

/// The churn report of `p`, with `f` or the defaults in its time zone.
async fn churn_report<H: Host>(
    host: &H,
    p: &Project,
    f: Option<Filters>,
) -> crate::Result<ChurnReport> {
    let f = f.unwrap_or_else(|| Filters {
        tz: p.timezone.clone(),
        ..Filters::default()
    });
    let map = map(host, &p.id).await;
    crate::churn(host.query(), &p.id, map.as_ref(), &f).await
}

/// `explore.churn`: who left, who is at risk now and why (#247).
pub struct Churn;

impl Command for Churn {
    const NAME: &'static str = "explore.churn";
    const ACCESS: Access = Access::Read;
    type Input = ChurnInput;
    type Output = ChurnReport;

    async fn run<H: Host>(host: &H, _: &Caller, input: ChurnInput) -> Result<ChurnReport, Error> {
        let p = project(host, &input.project).await?;
        churn_report(host, &p, input.filters)
            .await
            .map_err(unavailable)
    }
}

/// A flag made from a list of users (#269): how many of the treated and
/// of the held-out users were active in the last 7 days.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlagAudience {
    /// Flag key.
    pub flag: String,
    /// Switched on.
    pub enabled: bool,
    /// Listed users who get it.
    pub treated: i64,
    /// Of them, active in the last 7 days.
    pub treated_active: i64,
    /// Listed users held out (control).
    pub held: i64,
    /// Of them, active in the last 7 days.
    pub held_active: i64,
}

/// Count the treated and held-out users of `f`, and those of them in
/// `active`.
#[must_use]
pub fn flag_audience(
    f: &openlens_flags::Flag,
    active: &std::collections::HashSet<String>,
) -> FlagAudience {
    let (mut t, mut ta, mut h, mut ha) = (0i64, 0i64, 0i64, 0i64);
    for u in &f.users {
        let a = i64::from(active.contains(u));
        if f.held_out(u) {
            h += 1;
            ha += a;
        } else {
            t += 1;
            ta += a;
        }
    }
    FlagAudience {
        flag: f.key.clone(),
        enabled: f.enabled,
        treated: t,
        treated_active: ta,
        held: h,
        held_active: ha,
    }
}

/// `explore.flag_audiences`: the first five flags made from a list of
/// users (the users at risk, #269), with how many were active lately.
pub struct FlagAudiences;

impl Command for FlagAudiences {
    const NAME: &'static str = "explore.flag_audiences";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Vec<FlagAudience>;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: ProjectInput,
    ) -> Result<Vec<FlagAudience>, Error> {
        let project = input.project.as_str();
        let flags: Vec<openlens_flags::Flag> = host
            .meta()
            .flags(project)
            .map(|rows| {
                rows.iter()
                    .filter_map(|j| serde_json::from_str(j).ok())
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        for f in flags.into_iter().filter(|f| !f.users.is_empty()).take(5) {
            let mut params = openlens_sqlgen::Params::new();
            let ids: Vec<String> = f.users.iter().filter_map(|u| params.bind(u).ok()).collect();
            if ids.is_empty() {
                continue;
            }
            let since = (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d");
            let sql = format!(
                "SELECT DISTINCT distinct_id FROM events WHERE date >= '{since}' AND distinct_id IN ({})",
                ids.join(", ")
            );
            let active: std::collections::HashSet<String> = host
                .query()
                .with_params(&params)
                .sql(project, &sql)
                .await
                .map(|r| {
                    r.rows
                        .iter()
                        .filter_map(|x| {
                            x.get("distinct_id")
                                .and_then(|v| v.as_str())
                                .map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(flag_audience(&f, &active));
        }
        Ok(out)
    }
}

/// The input of `explore.make_audience`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MakeAudienceInput {
    /// Project id.
    pub project: String,
    /// Share of the users kept off as a control group (default 20, at
    /// most 50).
    #[serde(default)]
    pub holdout: Option<u8>,
}

/// The flag made.
#[derive(Debug, Clone, Serialize)]
pub struct MadeFlag {
    /// Its key.
    pub key: String,
}

/// `explore.make_audience`: a flag, off, listing the users at risk now
/// with a control group, for a win-back nudge behind
/// `openlens.isEnabled(key)` (#269).
///
/// Errors: [`Error::Unavailable`] when the churn report cannot be
/// computed, [`Error::Invalid`] when nobody is at risk, [`Error::Storage`]
/// when the flag is refused or cannot be stored.
pub struct MakeAudience;

impl Command for MakeAudience {
    const NAME: &'static str = "explore.make_audience";
    const ACCESS: Access = Access::Write;
    type Input = MakeAudienceInput;
    type Output = MadeFlag;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: MakeAudienceInput,
    ) -> Result<MadeFlag, Error> {
        let p = project(host, &input.project).await?;
        let Ok(c) = churn_report(host, &p, None).await else {
            return Err(Error::Unavailable(
                "The churn report is unavailable.".into(),
            ));
        };
        if c.at_risk.is_empty() {
            return Err(Error::Invalid("No user is at risk right now.".into()));
        }
        let key = format!("win-back-{}", chrono::Utc::now().format("%y%m%d"));
        let flag = openlens_flags::Flag {
            key: key.clone(),
            description: format!("Win-back nudge for {} users at risk", c.at_risk.len()),
            enabled: false,
            rollout: 100,
            roles: Vec::new(),
            accounts: Vec::new(),
            variants: Vec::new(),
            rules: Vec::new(),
            users: c.at_risk.iter().map(|a| a.distinct_id.clone()).collect(),
            holdout: input.holdout.unwrap_or(20).min(50),
        };
        flag.validate().map_err(Error::Storage)?;
        let json = serde_json::to_string(&flag).map_err(|e| Error::Storage(e.to_string()))?;
        host.meta()
            .save_flag(&p.id, &flag.key, &json)
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(MadeFlag { key })
    }
}

/// `explore.vitals`: every Web Vital explained (#408), for agents.
pub struct Vitals;

impl Command for Vitals {
    const NAME: &'static str = "explore.vitals";
    const ACCESS: Access = Access::Admin;
    type Input = FilteredInput;
    type Output = Vec<VitalReport>;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: FilteredInput,
    ) -> Result<Vec<VitalReport>, Error> {
        let p = project(host, &input.project).await?;
        let map = map(host, &p.id).await;
        crate::vitals(host.query(), &p.id, &input.filters, map.as_ref())
            .await
            .map_err(unavailable)
    }
}

/// `explore.audience`: devices, browsers, countries and languages (#225).
pub struct GetAudience;

impl Command for GetAudience {
    const NAME: &'static str = "explore.audience";
    const ACCESS: Access = Access::Read;
    type Input = FilteredInput;
    type Output = Audience;

    async fn run<H: Host>(host: &H, _: &Caller, input: FilteredInput) -> Result<Audience, Error> {
        let p = project(host, &input.project).await?;
        let map = map(host, &p.id).await;
        crate::audience(host.query(), &p.id, map.as_ref(), &input.filters)
            .await
            .map_err(unavailable)
    }
}

/// The input of `explore.flows`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowsInput {
    /// Project id.
    pub project: String,
    /// The board filters.
    pub filters: Filters,
    /// Start at the first use of this feature.
    #[serde(default)]
    pub from: Option<String>,
    /// `success`: only sessions that reached a success event.
    #[serde(default)]
    pub to: Option<String>,
}

/// The flow query of `from` (blank: none) and `to` (`success` or not).
#[must_use]
pub fn flow_query(from: Option<&str>, to: Option<&str>) -> FlowQuery {
    FlowQuery {
        from: from.filter(|s| !s.trim().is_empty()).map(str::to_owned),
        to_success: to == Some("success"),
    }
}

/// `explore.flows`: how sessions move between features (#224).
pub struct Flows;

impl Command for Flows {
    const NAME: &'static str = "explore.flows";
    const ACCESS: Access = Access::Read;
    type Input = FlowsInput;
    type Output = Flow;

    async fn run<H: Host>(host: &H, _: &Caller, input: FlowsInput) -> Result<Flow, Error> {
        let p = project(host, &input.project).await?;
        let fq = flow_query(input.from.as_deref(), input.to.as_deref());
        let map = map(host, &p.id).await;
        crate::flows(host.query(), &p.id, map.as_ref(), &input.filters, &fq)
            .await
            .map_err(unavailable)
    }
}

/// `explore.live`: what is happening now (#228).
pub struct GetLive;

impl Command for GetLive {
    const NAME: &'static str = "explore.live";
    const ACCESS: Access = Access::Read;
    type Input = FilteredInput;
    type Output = Live;

    async fn run<H: Host>(host: &H, _: &Caller, input: FilteredInput) -> Result<Live, Error> {
        let p = project(host, &input.project).await?;
        let map = map(host, &p.id).await;
        crate::live(host.query(), &p.id, map.as_ref(), &input.filters)
            .await
            .map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquisition_groups_by_channel_unless_asked() {
        assert_eq!(acquisition_by(None), "channel");
        assert_eq!(acquisition_by(Some("source")), "source");
        assert_eq!(acquisition_by(Some("referrer")), "referrer");
        assert_eq!(acquisition_by(Some("nope")), "channel");
    }

    #[test]
    fn flow_query_ignores_a_blank_start() {
        let q = flow_query(Some("  "), Some("success"));
        assert!(q.from.is_none() && q.to_success);
        let q = flow_query(Some("editor"), None);
        assert_eq!(q.from.as_deref(), Some("editor"));
        assert!(!q.to_success);
    }

    #[test]
    fn flag_audience_splits_treated_and_held_out() {
        let f = openlens_flags::Flag {
            key: "win-back".into(),
            description: String::new(),
            enabled: true,
            rollout: 100,
            roles: Vec::new(),
            accounts: Vec::new(),
            variants: Vec::new(),
            rules: Vec::new(),
            users: (0..20).map(|i| format!("u{i}")).collect(),
            holdout: 50,
        };
        let active: std::collections::HashSet<String> = f.users.iter().take(10).cloned().collect();
        let a = flag_audience(&f, &active);
        assert_eq!(a.treated + a.held, 20);
        assert_eq!(a.treated_active + a.held_active, 10);
        let held = f.users.iter().filter(|u| f.held_out(u)).count();
        assert_eq!(a.held, i64::try_from(held).unwrap_or_default());
    }
}
