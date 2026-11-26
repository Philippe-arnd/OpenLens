//! The deploy notification formats the hook reads (#504, D-053), each
//! from the platform's public documentation (`docs/clean-room.md`). The
//! format is told by a header when the platform sends one, otherwise by
//! the body's shape; the plain form comes last.

use serde_json::Value;

use super::{Notice, Status};

/// The request headers that tell a format apart.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hints<'a> {
    /// `X-GitHub-Event`.
    pub github_event: Option<&'a str>,
    /// `Gotk-Component` (Flux's notification controller).
    pub flux_component: Option<&'a str>,
}

/// What a notification says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// A deploy to record.
    Deploy(Notice),
    /// A notification of the platform that is not a deploy (a ping, a
    /// build event…): accepted, not recorded. The text says which.
    Ignored(&'static str),
}

/// Read a notification. `Err` when no format matches or a field is wrong;
/// the text says why, for whoever wires the hook.
///
/// # Errors
/// An unknown shape, or a plain form without a known `status`.
pub fn parse(hints: Hints<'_>, body: &Value) -> Result<Parsed, &'static str> {
    if !body.is_object() {
        return Err("the body must be a JSON object");
    }
    if let Some(ev) = hints.github_event {
        return Ok(github(ev, body));
    }
    if hints.flux_component.is_some()
        || (body.get("involvedObject").is_some_and(Value::is_object)
            && str_at(body, &["reportingController"]).is_some())
    {
        return Ok(flux(body));
    }
    if let Some(t) = str_at(body, &["type"])
        && let Some(kind) = t.strip_prefix("deployment.")
        && body.get("payload").is_some_and(Value::is_object)
    {
        return Ok(vercel(kind, body));
    }
    if str_at(body, &["data", "serviceId"]).is_some() {
        return Ok(render(body));
    }
    if let Some(ev) = str_at(body, &["event"])
        && ev.starts_with("deployment_")
        && (body.get("application_uuid").is_some() || body.get("deployment_uuid").is_some())
    {
        return Ok(coolify(ev, body));
    }
    if str_at(body, &["site_id"]).is_some()
        && let Some(state) = str_at(body, &["state"])
    {
        return Ok(netlify(state, body));
    }
    if body.get("app").is_some_and(Value::is_object) || str_at(body, &["phase"]).is_some() {
        return argocd(body);
    }
    if str_at(body, &["FLY_APP_NAME"]).is_some() || str_at(body, &["FLY_IMAGE_REF"]).is_some() {
        return fly(body);
    }
    if body.get("status").is_some() {
        return plain(body);
    }
    Err(
        "not a known deploy notification: send Coolify, Vercel, Netlify, Render, GitHub \
         deployment_status, Argo CD, Flux or Fly notifications, or {\"status\": \"succeeded\", \
         \"release\"?, \"commit\"?, \"environment\"?, \"at\"?}",
    )
}

/// The string at `path`, when it is a non-empty string.
pub(super) fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// A time: RFC 3339, or a number of seconds (milliseconds past 10^11).
pub(super) fn time(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => {
            let n = n.as_f64()?;
            let secs = if n > 1e11 { n / 1000.0 } else { n };
            (secs.is_finite() && secs > 0.0).then_some(secs as i64)
        }
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|t| t.timestamp()),
        _ => None,
    }
}

fn notice(source: &'static str, status: Status) -> Notice {
    Notice {
        source,
        status,
        environment: None,
        release: None,
        commit: None,
        at: None,
    }
}

fn owned(s: Option<&str>) -> Option<String> {
    s.map(str::to_owned)
}

/// GitHub `deployment_status`: `deployment_status.state`,
/// `deployment.environment`, `deployment.sha`.
fn github(event: &str, b: &Value) -> Parsed {
    if event != "deployment_status" {
        return Parsed::Ignored("a GitHub event other than deployment_status");
    }
    let status = match str_at(b, &["deployment_status", "state"]) {
        Some("queued" | "pending" | "in_progress") => Status::Started,
        Some("success") => Status::Succeeded,
        Some("failure" | "error") => Status::Failed,
        _ => return Parsed::Ignored("a GitHub deployment state that is not a deploy step"),
    };
    let mut n = notice("github", status);
    n.environment = owned(str_at(b, &["deployment", "environment"]));
    n.commit = owned(str_at(b, &["deployment", "sha"]));
    n.at = time(
        b.get("deployment_status")
            .and_then(|s| s.get("updated_at").or_else(|| s.get("created_at"))),
    );
    Parsed::Deploy(n)
}

/// Vercel: `type` `deployment.*`, `payload.target`, the commit in
/// `payload.deployment.meta`, `createdAt` in milliseconds.
fn vercel(kind: &str, b: &Value) -> Parsed {
    let status = match kind {
        "created" => Status::Started,
        "succeeded" | "ready" | "promoted" => Status::Succeeded,
        "error" | "canceled" => Status::Failed,
        _ => return Parsed::Ignored("a Vercel deployment event that is not a deploy step"),
    };
    let mut n = notice("vercel", status);
    n.environment = Some(match str_at(b, &["payload", "target"]) {
        Some(t @ ("production" | "staging")) => t.to_owned(),
        _ => "preview".to_owned(),
    });
    n.commit = ["githubCommitSha", "gitlabCommitSha", "bitbucketCommitSha"]
        .iter()
        .find_map(|k| str_at(b, &["payload", "deployment", "meta", k]))
        .map(str::to_owned);
    n.at = time(b.get("createdAt"));
    Parsed::Deploy(n)
}

/// Netlify: the deploy object (`state`, `context`, `branch`,
/// `commit_ref`, `published_at`).
fn netlify(state: &str, b: &Value) -> Parsed {
    let status = match state {
        "ready" => Status::Succeeded,
        "error" => Status::Failed,
        "new" | "enqueued" | "building" | "uploading" | "uploaded" | "preparing" | "prepared"
        | "processing" | "processed" => Status::Started,
        _ => return Parsed::Ignored("a Netlify deploy state that is not a deploy step"),
    };
    let mut n = notice("netlify", status);
    n.environment = Some(
        match (str_at(b, &["context"]), str_at(b, &["branch"])) {
            (Some("production"), _) => "production",
            (Some("branch-deploy"), Some("staging" | "stage")) => "staging",
            _ => "preview",
        }
        .to_owned(),
    );
    n.commit = owned(str_at(b, &["commit_ref"]));
    n.at = time(
        b.get("published_at")
            .filter(|v| !v.is_null())
            .or_else(|| b.get("updated_at"))
            .or_else(|| b.get("created_at")),
    );
    Parsed::Deploy(n)
}

/// Render: `type` `deploy_started` / `deploy_ended`, `data.status`,
/// `timestamp`. No environment and no commit.
fn render(b: &Value) -> Parsed {
    let status = match (str_at(b, &["type"]), str_at(b, &["data", "status"])) {
        (Some("deploy_started"), _) => Status::Started,
        (Some("deploy_ended"), Some("succeeded")) => Status::Succeeded,
        (Some("deploy_ended"), Some("failed" | "canceled")) => Status::Failed,
        _ => return Parsed::Ignored("a Render event that is not a deploy step"),
    };
    let mut n = notice("render", status);
    n.at = time(b.get("timestamp"));
    Parsed::Deploy(n)
}

/// Coolify: `event` `deployment_success` / `deployment_failed`,
/// `environment`; a preview when `pull_request_id` is set. No commit.
fn coolify(event: &str, b: &Value) -> Parsed {
    let status = match event {
        "deployment_success" => Status::Succeeded,
        "deployment_failed" => Status::Failed,
        _ => return Parsed::Ignored("a Coolify event that is not a deploy result"),
    };
    let mut n = notice("coolify", status);
    let preview = b
        .get("pull_request_id")
        .is_some_and(|v| !v.is_null() && v != 0);
    n.environment = if preview {
        Some("preview".to_owned())
    } else {
        owned(str_at(b, &["environment"]))
    };
    Parsed::Deploy(n)
}

/// Flux (generic provider): `severity`, `reason`, the revision in
/// `metadata` (`<branch>@sha1:<hash>` or `<branch>/<hash>`), and the
/// `environment` and `release` an Alert's `eventMetadata` may add.
fn flux(b: &Value) -> Parsed {
    let severity = str_at(b, &["severity"]).unwrap_or("info");
    let reason = str_at(b, &["reason"]).unwrap_or("");
    let status = if severity == "error" {
        Status::Failed
    } else if reason.contains("Rollback") {
        Status::RolledBack
    } else if reason.ends_with("Succeeded") {
        Status::Succeeded
    } else if reason == "Progressing" {
        Status::Started
    } else {
        return Parsed::Ignored("a Flux event that is not a deploy step");
    };
    let mut n = notice("flux", status);
    let meta = b.get("metadata").and_then(Value::as_object);
    if let Some(m) = meta {
        n.commit = m
            .iter()
            .find(|(k, _)| k.as_str() == "revision" || k.ends_with("/revision"))
            .and_then(|(_, v)| v.as_str())
            .map(revision_hash);
        n.environment = m
            .get("environment")
            .and_then(Value::as_str)
            .map(str::to_owned);
        n.release = m.get("release").and_then(Value::as_str).map(str::to_owned);
    }
    n.at = time(b.get("timestamp"));
    Parsed::Deploy(n)
}

/// The hash of a Flux or Argo CD revision: what follows the last `:` or
/// `/` (`main@sha1:abc…`, `main/abc…`).
fn revision_hash(r: &str) -> String {
    r.rsplit([':', '/']).next().unwrap_or(r).to_owned()
}

/// Argo CD notifications: the body is a template, so two are read: the
/// one `docs/agents/wire.md` gives (`phase`, `revision`, `at`,
/// `environment`, `release`), or `{"app": {{toJson .app}}}` (the
/// Application's `status.operationState` and `status.sync.revision`).
fn argocd(b: &Value) -> Result<Parsed, &'static str> {
    let app = b.get("app").filter(|a| a.is_object());
    let phase = str_at(b, &["phase"])
        .or_else(|| app.and_then(|a| str_at(a, &["status", "operationState", "phase"])));
    let status = match phase {
        Some("Running") => Status::Started,
        Some("Succeeded") => Status::Succeeded,
        Some("Failed" | "Error") => Status::Failed,
        Some(_) => {
            return Ok(Parsed::Ignored(
                "an Argo CD phase that is not a deploy step",
            ));
        }
        None => return Err("an Argo CD notification needs `phase`"),
    };
    let mut n = notice("argocd", status);
    n.commit = str_at(b, &["revision"])
        .or_else(|| app.and_then(|a| str_at(a, &["status", "sync", "revision"])))
        .map(revision_hash);
    n.environment = owned(str_at(b, &["environment"]));
    n.release = owned(str_at(b, &["release"]));
    n.at = time(b.get("at")).or_else(|| {
        let op = app?.get("status")?.get("operationState")?;
        time(op.get("finishedAt").or_else(|| op.get("startedAt")))
    });
    Ok(Parsed::Deploy(n))
}

/// Fly.io sends no deploy notification, so a step after `fly deploy` (or
/// the app at start) posts the machine's variables: `FLY_IMAGE_REF`'s tag
/// is the release unless `release` is given; `status` defaults to
/// succeeded.
fn fly(b: &Value) -> Result<Parsed, &'static str> {
    let status = match str_at(b, &["status"]) {
        None => Status::Succeeded,
        Some(s) => Status::parse(s).ok_or("unknown `status`")?,
    };
    let mut n = notice("fly", status);
    n.release = owned(str_at(b, &["release"])).or_else(|| {
        let image = str_at(b, &["FLY_IMAGE_REF"])?;
        let tag = image.rsplit_once(':').map(|(_, t)| t)?;
        (!image.contains('@') && !tag.contains('/')).then(|| tag.to_owned())
    });
    n.commit = owned(str_at(b, &["commit"]));
    n.environment = owned(str_at(b, &["environment"]));
    n.at = time(b.get("at"));
    Ok(Parsed::Deploy(n))
}

/// The plain form: `{status, release?, commit?, environment?, at?}`.
fn plain(b: &Value) -> Result<Parsed, &'static str> {
    let status = str_at(b, &["status"])
        .and_then(Status::parse)
        .ok_or("`status` must be started, succeeded, failed or rolled_back")?;
    let mut n = notice("plain", status);
    n.release = owned(str_at(b, &["release"]));
    n.commit = owned(str_at(b, &["commit"]));
    n.environment = owned(str_at(b, &["environment"]));
    n.at = time(b.get("at"));
    Ok(Parsed::Deploy(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deploy(h: Hints<'_>, b: Value) -> Notice {
        match parse(h, &b) {
            Ok(Parsed::Deploy(n)) => n,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn times_are_read_in_seconds_milliseconds_or_rfc_3339() {
        let s = time(Some(&json!(1_764_000_000))).unwrap();
        assert_eq!(time(Some(&json!(1_764_000_000_123_i64))), Some(s));
        assert_eq!(
            time(Some(&json!("2025-11-24T16:00:00.979294509Z"))),
            Some(s)
        );
        assert_eq!(time(Some(&json!("yesterday"))), None);
        assert_eq!(time(Some(&json!(-5))), None);
    }

    #[test]
    fn a_flux_rollback_and_revision_are_read() {
        let n = deploy(
            Hints::default(),
            json!({"involvedObject": {"kind": "HelmRelease"}, "reportingController": "helm-controller",
                   "severity": "info", "reason": "RollbackSucceeded",
                   "metadata": {"helm.toolkit.fluxcd.io/revision": "main@sha1:0123456789abcdef", "environment": "staging"}}),
        );
        assert_eq!(n.status, Status::RolledBack);
        assert_eq!(n.commit.as_deref(), Some("0123456789abcdef"));
        assert_eq!(n.environment.as_deref(), Some("staging"));
    }

    #[test]
    fn notifications_that_are_not_deploys_are_ignored_and_unknown_shapes_refused() {
        let ping = parse(
            Hints {
                github_event: Some("ping"),
                ..Hints::default()
            },
            &json!({"zen": "x"}),
        );
        assert!(matches!(ping, Ok(Parsed::Ignored(_))));
        let build = parse(
            Hints::default(),
            &json!({"type": "build_started", "data": {"serviceId": "srv-1"}}),
        );
        assert!(matches!(build, Ok(Parsed::Ignored(_))));
        assert!(parse(Hints::default(), &json!({"hello": 1})).is_err());
        assert!(parse(Hints::default(), &json!({"status": "maybe"})).is_err());
        assert!(parse(Hints::default(), &json!([1])).is_err());
    }
}
