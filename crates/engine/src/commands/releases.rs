//! Releases as commands (#500, D-049): each release per environment with
//! its state (rolling out, live, superseded, rolled back), first and last
//! seen, users and time in staging, and the deploys the deploy hook
//! recorded (#504). The rules are in [`crate::releases`].
//!
//! Each release of the environment judged also has its verdict (#522,
//! D-067): `releases.list` carries it with its reasons, `releases.report`
//! the whole comparison of one release ([`crate::verdict`]).
//!
//! Both carry the release notes the git host gave (#574, D-093):
//! `releases.list` those of the newest releases recorded with a range,
//! `releases.report` the release's own, or why they are not there. The
//! notes carry the features each release touched (#576), which the
//! verdict judges and lists first.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::board_verdict::LatestRelease;
use crate::releases::{self, Releases};
use crate::verdict::Report;
use crate::verdict_read::{self, Scope};

/// The input of `releases.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// One environment only; every environment when absent.
    #[serde(default)]
    pub environment: Option<String>,
}

/// `releases.list`: the project's releases over the last 90 days.
pub struct List;

fn engine_err(e: crate::EngineError) -> Error {
    match e {
        crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
        _ => Error::Unavailable(e.to_string()),
    }
}

/// The releases of `input`, without verdicts, and the time zone of their
/// times.
async fn list<H: Host>(
    host: &H,
    caller: &Caller,
    input: &ListInput,
) -> Result<(Releases, openlens_meta::Project), Error> {
    let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
    let tz = caller.tz.clone().unwrap_or_else(|| p.timezone.clone());
    let builds = host
        .meta()
        .release_builds(&p.id)
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let env = input
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    let since = chrono::Utc::now().timestamp() - (releases::WINDOW_DAYS + 1) * 86_400;
    let deploys = host
        .meta()
        .deploys(&p.id, since)
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let hook = host
        .meta()
        .deploy_hook_since(&p.id)
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let mut r = releases::releases(host.query(), &p.id, &tz, env, &builds, &deploys)
        .await
        .map_err(engine_err)?;
    r.deploy_hook = hook.map(|t| releases::local(t as f64, &tz));
    Ok((r, p))
}

/// The verdicts of the releases of `environment` in `r`.
async fn judge<H: Host>(
    host: &H,
    project: &str,
    environment: &str,
    r: &Releases,
) -> Result<Vec<Report>, Error> {
    let map = host.feature_map(project).await;
    let goals = super::goals::load(host.meta(), project);
    // Keys as the current map names them (a feature renamed since keeps
    // its place).
    let current_key = |k: String| match &map {
        Some((m, _)) => m.current_key(&k).map_or_else(|| k.clone(), str::to_owned),
        None => k,
    };
    let touched: HashMap<String, Vec<String>> =
        crate::release_features::by_release(host.meta(), project, environment)
            .into_iter()
            .map(|(release, fs)| {
                (
                    release,
                    fs.into_iter().map(|f| current_key(f.key)).collect(),
                )
            })
            .collect();
    let scope = Scope {
        environment,
        tz: &r.timezone,
        map: map.as_ref().map(|(m, v)| (m, *v)),
        goals: &goals,
        touched: &touched,
    };
    verdict_read::judge(
        host.query(),
        project,
        scope,
        &r.releases,
        chrono::Utc::now(),
    )
    .await
    .map_err(engine_err)
}

impl Command for List {
    const NAME: &'static str = "releases.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Releases;

    async fn run<H: Host>(host: &H, caller: &Caller, input: ListInput) -> Result<Releases, Error> {
        let (mut r, p) = list(host, caller, &input).await?;
        r.notes = crate::release_git::listed(host, &p.id);
        // One environment is judged: the one asked for, or production.
        let judged = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(releases::PRODUCTION)
            .to_owned();
        let reports = judge(host, &p.id, &judged, &r).await?;
        // The newest judged release by layer, with its routes that got
        // worse, their top server error and sessions (#623).
        if let Some(newest) = reports.last() {
            r.latest = Some(
                crate::verdict_backend::latest(
                    host.query(),
                    host.data_dir(),
                    &p.id,
                    &judged,
                    newest,
                )
                .await,
            );
        }
        for x in &mut r.releases {
            if x.environment != judged {
                if x.users > 0 {
                    x.reasons = vec![format!(
                        "Judged in one environment at a time: list {} alone for its verdicts.",
                        x.environment
                    )];
                }
                continue;
            }
            if let Some(rep) = reports.iter().find(|y| y.release == x.release) {
                x.verdict = rep.verdict;
                x.reasons = rep.reasons.clone();
            }
        }
        Ok(r)
    }
}

/// The input of `releases.report`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportInput {
    /// Project id.
    pub project: String,
    /// Release name, as `releases.list` shows it.
    pub release: String,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
}

/// `releases.report`: one release compared with the release before it in
/// its environment: the verdict, its reasons and every number compared.
pub struct ReleaseReport;

impl Command for ReleaseReport {
    const NAME: &'static str = "releases.report";
    const ACCESS: Access = Access::Read;
    type Input = ReportInput;
    type Output = Report;

    async fn run<H: Host>(host: &H, caller: &Caller, input: ReportInput) -> Result<Report, Error> {
        let env = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(releases::PRODUCTION)
            .to_owned();
        let release = input.release.trim().to_owned();
        if release.is_empty() || release.len() > 200 {
            return Err(Error::Invalid(
                "Name a release (200 characters at most).".into(),
            ));
        }
        let list_input = ListInput {
            project: input.project,
            environment: Some(env.clone()),
        };
        let (r, p) = list(host, caller, &list_input).await?;
        if !r
            .releases
            .iter()
            .any(|x| x.release == release && x.environment == env && x.users > 0)
        {
            return Err(Error::Invalid(format!(
                "No release {release} with traffic in {env} over the last {} days.",
                releases::WINDOW_DAYS
            )));
        }
        let reports = judge(host, &p.id, &env, &r).await?;
        let mut report = reports
            .into_iter()
            .find(|x| x.release == release)
            .ok_or_else(|| Error::Invalid(format!("No release {release} in {env}.")))?;
        report.notes = Some(crate::release_git::notes(host, &p.id, &env, &release));
        Ok(report)
    }
}

/// The verdict of the newest release of `environment` with traffic
/// (#614): what the board's status strip and lead read. The server judges
/// it next to the board and caches it with it; a failure is said, not
/// hidden.
pub async fn latest<H: Host>(host: &H, project: &str, environment: &str) -> LatestRelease {
    let env = Some(environment.trim())
        .filter(|e| !e.is_empty())
        .unwrap_or(releases::PRODUCTION)
        .to_owned();
    let input = ListInput {
        project: project.to_owned(),
        environment: Some(env.clone()),
    };
    let judged = async {
        let (r, p) = list(host, &Caller::default(), &input).await?;
        // Oldest first: the last one is the newest.
        Ok::<_, Error>(judge(host, &p.id, &env, &r).await?.pop())
    }
    .await;
    match judged {
        Ok(Some(r)) => LatestRelease::Judged(Box::new(r)),
        Ok(None) => LatestRelease::NoRelease,
        Err(e) => LatestRelease::Unavailable {
            reason: e.to_string(),
        },
    }
}
