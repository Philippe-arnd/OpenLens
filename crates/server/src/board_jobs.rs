//! Long boards as background jobs (#515, D-063).
//!
//! A date range or past release that reads more than 30 days of events
//! asks first (D-061); its "Run it", and `POST /api/projects/{id}/board/jobs`,
//! start the board here instead of in the request:
//!
//! - a job is queued, then running, then done, failed or cancelled;
//! - at most [`RUNNING`] jobs compute at once per instance, the others wait
//!   their turn (queued); at most [`ACTIVE`] are queued or running, and one
//!   per person: beyond, starting one is refused;
//! - a job is its starter's alone (option A of #325): anyone else gets a
//!   404, as for a job that does not exist;
//! - a finished job is kept for [`KEEP`], at most [`CAPACITY`] jobs in all;
//! - cancelling aborts the job's task, which drops its queries, and a
//!   dropped query stops on the query runtime (`openlens_query`).
//!
//! The board itself is computed through the board cache
//! ([`crate::boards::with_revenue`]), so a job and a request for the same
//! filters share one computation, and a rollup pass still waits for both.
//! The registry lives in [`crate::cache::Caches`], not in a static.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openlens_engine::{Board, Filters};
use openlens_featuremap::FeatureMap;
use serde::Serialize;

use crate::AppState;
use crate::boards::RevenueResult;

mod http;

/// The dashboard's and the API's job routes.
pub(crate) fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/p/{project}/board/jobs", post(http::page_start))
        .route("/p/{project}/board/jobs/{job}", get(http::page))
        .route(
            "/p/{project}/board/jobs/{job}/cancel",
            post(http::page_cancel),
        )
        .route("/api/projects/{project}/board/jobs", post(http::api_start))
        .route(
            "/api/projects/{project}/board/jobs/{job}",
            get(http::api_get).delete(http::api_cancel),
        )
}

/// Jobs computing at once per instance.
pub(crate) const RUNNING: usize = 2;
/// Jobs queued or running at once per instance.
pub(crate) const ACTIVE: usize = 8;
/// How long a finished job is kept.
pub(crate) const KEEP: Duration = Duration::from_secs(3600);
/// Jobs kept at all, finished ones included.
pub(crate) const CAPACITY: usize = 256;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    /// Queued or running.
    pub(crate) fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// The name the API and the page use.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One job, as the registry keeps it.
#[derive(Clone)]
pub(crate) struct Job {
    pub(crate) id: String,
    /// Who started it: a user id, `owner`, or `token:<hash>` for a scoped
    /// API token.
    pub(crate) owner: String,
    pub(crate) project: String,
    pub(crate) filters: Filters,
    /// The board's parameters, to go back to its page.
    pub(crate) params: Vec<(&'static str, String)>,
    /// The window, said.
    pub(crate) label: String,
    pub(crate) state: JobState,
    /// The estimate's rough time (D-061), 0 when there is none.
    pub(crate) estimate_secs: f64,
    started: Option<Instant>,
    ended: Option<Instant>,
    pub(crate) result: Option<Arc<(Board, RevenueResult)>>,
    pub(crate) error: Option<String>,
    abort: Option<tokio::task::AbortHandle>,
}

impl Job {
    /// Seconds spent computing (0 while queued).
    pub(crate) fn elapsed_secs(&self) -> f64 {
        self.started.map_or(0.0, |s| {
            self.ended
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(s)
                .as_secs_f64()
        })
    }

    /// From 0 to 1: 1 once done; while running, the time spent against
    /// the estimate, never more than 0.95 (the board runs its queries side
    /// by side, so it cannot count its days cheaply; the estimate is
    /// rough).
    pub(crate) fn progress(&self) -> f64 {
        match self.state {
            JobState::Done => 1.0,
            JobState::Running if self.estimate_secs > 0.0 => {
                (self.elapsed_secs() / self.estimate_secs).clamp(0.0, 0.95)
            }
            _ => 0.0,
        }
    }
}

/// Why a job was not started.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The person already has this job queued or running.
    Yours(String),
    /// The instance has as many jobs as it takes.
    Full,
}

/// What a new job computes, and for whom.
pub(crate) struct NewJob {
    pub(crate) owner: String,
    pub(crate) project: String,
    pub(crate) filters: Filters,
    pub(crate) map: Option<(FeatureMap, i64)>,
    pub(crate) params: Vec<(&'static str, String)>,
    pub(crate) label: String,
    pub(crate) estimate_secs: f64,
}

/// Every board job of the instance.
pub struct BoardJobs {
    jobs: Mutex<HashMap<String, Job>>,
    /// One permit per running job.
    slots: Arc<tokio::sync::Semaphore>,
}

impl Default for BoardJobs {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(RUNNING)),
        }
    }
}

impl BoardJobs {
    /// Jobs kept now, finished ones included.
    pub(crate) fn len(&self) -> usize {
        self.jobs.lock().map_or(0, |m| m.len())
    }

    /// The job `id`, if `owner` started it and it is still kept.
    pub(crate) fn get(&self, owner: &str, id: &str) -> Option<Job> {
        let mut m = self.jobs.lock().ok()?;
        purge(&mut m, Instant::now());
        m.get(id).filter(|j| j.owner == owner).cloned()
    }

    /// Stop the job `id` of `owner`: its task is aborted, which stops its
    /// queries. A finished job stays as it was. `None`: no such job of
    /// theirs.
    pub(crate) fn cancel(&self, owner: &str, id: &str) -> Option<JobState> {
        let mut m = self.jobs.lock().ok()?;
        let j = m.get_mut(id).filter(|j| j.owner == owner)?;
        if j.state.active() {
            if let Some(a) = j.abort.take() {
                a.abort();
            }
            j.state = JobState::Cancelled;
            j.ended = Some(Instant::now());
        }
        Some(j.state)
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Ok(mut m) = self.jobs.lock()
            && let Some(j) = m.get_mut(id)
        {
            f(j);
        }
    }
}

/// Drop the jobs finished more than [`KEEP`] ago, then the oldest finished
/// ones while there are more than [`CAPACITY`].
fn purge(m: &mut HashMap<String, Job>, now: Instant) {
    m.retain(|_, j| {
        j.ended
            .is_none_or(|e| now.saturating_duration_since(e) < KEEP)
    });
    while m.len() > CAPACITY {
        let Some(oldest) = m
            .values()
            .filter(|j| !j.state.active())
            .min_by_key(|j| j.ended)
            .map(|j| j.id.clone())
        else {
            break;
        };
        m.remove(&oldest);
    }
}

/// A new job id: 128 random bits, in hex.
fn new_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

/// Queue the board of `new` and return the job's id.
///
/// # Errors
/// [`Refused::Yours`] when its owner already has a job queued or running,
/// [`Refused::Full`] when the instance has [`ACTIVE`] of them.
pub(crate) fn start(state: &AppState, new: NewJob) -> Result<String, Refused> {
    let registry = &state.caches.board_jobs;
    let id = new_id();
    {
        let mut m = registry.jobs.lock().map_err(|_| Refused::Full)?;
        purge(&mut m, Instant::now());
        if let Some(j) = m.values().find(|j| j.owner == new.owner && j.state.active()) {
            return Err(Refused::Yours(j.id.clone()));
        }
        if m.values().filter(|j| j.state.active()).count() >= ACTIVE || m.len() >= CAPACITY {
            return Err(Refused::Full);
        }
        m.insert(
            id.clone(),
            Job {
                id: id.clone(),
                owner: new.owner,
                project: new.project.clone(),
                filters: new.filters.clone(),
                params: new.params,
                label: new.label,
                state: JobState::Queued,
                estimate_secs: new.estimate_secs,
                started: None,
                ended: None,
                result: None,
                error: None,
                abort: None,
            },
        );
    }
    let task = {
        let (state, id) = (state.clone(), id.clone());
        let slots = registry.slots.clone();
        let (project, filters, map) = (new.project, new.filters, new.map);
        async move {
            let jobs = &state.caches.board_jobs;
            let Ok(_turn) = slots.acquire_owned().await else {
                return;
            };
            let mut go = false;
            jobs.update(&id, |j| {
                if j.state == JobState::Queued {
                    j.state = JobState::Running;
                    j.started = Some(Instant::now());
                    go = true;
                }
            });
            if !go {
                return;
            }
            let map = map.as_ref().map(|(m, v)| (m, *v));
            let r = crate::boards::with_revenue(&state, &project, &filters, map).await;
            jobs.update(&id, |j| {
                if j.state != JobState::Running {
                    return;
                }
                match r {
                    Ok(done) => {
                        j.state = JobState::Done;
                        j.result = Some(Arc::new(done));
                    }
                    Err(e) => {
                        tracing::warn!(project, err = %e, "board job failed");
                        j.state = JobState::Failed;
                        j.error = Some(e.to_string());
                    }
                }
                j.ended = Some(Instant::now());
                j.abort = None;
            });
        }
    };
    let handle = tokio::spawn(task);
    registry.update(&id, |j| {
        if j.state.active() {
            j.abort = Some(handle.abort_handle());
        } else if j.state == JobState::Cancelled {
            handle.abort();
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, state: JobState, ended: Option<Instant>) -> Job {
        Job {
            id: id.to_owned(),
            owner: "u".into(),
            project: "p".into(),
            filters: Filters::default(),
            params: Vec::new(),
            label: String::new(),
            state,
            estimate_secs: 0.0,
            started: None,
            ended,
            result: None,
            error: None,
            abort: None,
        }
    }

    #[test]
    fn finished_jobs_go_after_an_hour_and_active_ones_stay() {
        let now = Instant::now();
        let Some(old) = now.checked_sub(KEEP + Duration::from_secs(1)) else {
            return; // a clock too young to go back an hour
        };
        let mut m = HashMap::new();
        m.insert("old".into(), job("old", JobState::Done, Some(old)));
        m.insert("new".into(), job("new", JobState::Done, Some(now)));
        m.insert("run".into(), job("run", JobState::Running, None));
        purge(&mut m, now);
        assert!(!m.contains_key("old"));
        assert!(m.contains_key("new") && m.contains_key("run"));
    }

    #[test]
    fn a_running_job_is_never_shown_as_done() {
        let mut j = job("j", JobState::Running, None);
        j.estimate_secs = 0.001;
        j.started = Some(Instant::now());
        assert!(j.progress() < 1.0);
        j.state = JobState::Done;
        assert!((j.progress() - 1.0).abs() < f64::EPSILON);
    }
}
