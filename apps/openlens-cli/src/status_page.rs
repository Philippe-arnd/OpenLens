//! `openlens-cli status-page` (#561, D-079): show or change the public
//! status page's settings (`status_page.get`, `status_page.set`), and read
//! the page itself as JSON.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::{Body, Client, out};

#[derive(Subcommand)]
pub(crate) enum StatusPageCmd {
    /// The settings: on or off, the address, the components and the
    /// environments that can be shown.
    Show {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Change the settings: only what is given changes.
    Set {
        /// Serve the page.
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Stop serving it (the address is kept).
        #[arg(long)]
        off: bool,
        /// The address, `/status/<slug>`: 3 to 40 of a-z, 0-9 and -.
        #[arg(long)]
        slug: Option<String>,
        /// A component as `environment=Public name` (production is always
        /// first); repeat for several. Given once, the list replaces the
        /// current one.
        #[arg(long = "component")]
        components: Vec<String>,
        /// Show this component's response time; repeat for several.
        #[arg(long = "response-time")]
        response_time: Vec<String>,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// The public page as JSON, what anyone reads at `/status/<slug>.json`.
    Public {
        /// The page's slug.
        slug: String,
    },
}

/// `environment=Public name` as a component.
fn component(raw: &str, response_time: &[String]) -> Result<Value, String> {
    let (env, name) = raw.split_once('=').unwrap_or((raw, ""));
    let env = env.trim();
    if env.is_empty() {
        return Err(format!("--component {raw:?}: environment=Public name"));
    }
    Ok(json!({
        "environment": env,
        "name": name.trim(),
        "response_time": response_time.iter().any(|r| r.trim() == env),
    }))
}

/// The body of `PUT /api/projects/{id}/status-page`.
pub(crate) fn set_body(
    on: bool,
    off: bool,
    slug: Option<&str>,
    components: &[String],
    response_time: &[String],
) -> Result<Value, String> {
    let mut body = serde_json::Map::new();
    if on || off {
        body.insert("enabled".into(), json!(on));
    }
    if let Some(s) = slug {
        body.insert("slug".into(), json!(s));
    }
    if !components.is_empty() {
        let list = components
            .iter()
            .map(|c| component(c, response_time))
            .collect::<Result<Vec<_>, _>>()?;
        body.insert("components".into(), Value::Array(list));
    }
    Ok(Value::Object(body))
}

/// A slug as a path segment: only the characters a slug may hold.
pub(crate) fn slug_path(slug: &str) -> Result<String, String> {
    if !slug.is_empty()
        && slug.len() <= 40
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Ok(format!("/status/{slug}.json"))
    } else {
        Err("a slug is 3 to 40 of a-z, 0-9 and -".into())
    }
}

/// Run one `status-page` subcommand: true when the server agreed.
pub(crate) fn run(c: &Client, cmd: &StatusPageCmd) -> Result<bool, String> {
    let at = |project: &str| format!("/api/projects/{project}/status-page");
    let (status, v): (u16, Value) = match cmd {
        StatusPageCmd::Show { project } => c.call("GET", &at(project), None)?,
        StatusPageCmd::Set {
            on,
            off,
            slug,
            components,
            response_time,
            project,
        } => {
            let body = set_body(*on, *off, slug.as_deref(), components, response_time)?;
            c.call("PUT", &at(project), Some(Body::Json(body)))?
        }
        StatusPageCmd::Public { slug } => c.call("GET", &slug_path(slug)?, None)?,
    };
    out(&v);
    Ok((200..300).contains(&status))
}
