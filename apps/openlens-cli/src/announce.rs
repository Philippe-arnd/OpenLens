//! `openlens-cli announce` (#562): status announcements and incident
//! updates from a terminal or a CI step ("deploying v2.3", then resolved).
//! Each subcommand calls the REST route of an `announcements.*` command.

use clap::Subcommand;
use serde_json::{Map, Value, json};

use crate::{Body, Client, out};

#[derive(Subcommand)]
pub(crate) enum AnnounceCmd {
    /// The announcements of the last days (90 by default), the pinned
    /// notice and what to do about recent incidents.
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
        /// Days back (1 to 400).
        #[arg(long)]
        days: Option<i64>,
    },
    /// Post an announcement; prints it with its id.
    Post {
        /// Title, one line.
        title: String,
        /// Text, plain (at most 4000 characters).
        #[arg(long, default_value = "")]
        body: String,
        /// information, degraded, outage or resolved.
        #[arg(long, default_value = "information")]
        level: String,
        /// production or an environment name (repeat for several).
        #[arg(long = "component")]
        components: Vec<String>,
        /// Start: RFC 3339, Unix seconds or `YYYY-MM-DD HH:MM` in the
        /// project's zone (default now).
        #[arg(long)]
        start: Option<String>,
        /// End, the same way.
        #[arg(long)]
        end: Option<String>,
        /// Pin it at the top (information only).
        #[arg(long)]
        pin: bool,
        /// The incident it follows (id from `announce list`).
        #[arg(long)]
        incident: Option<i64>,
        /// Its first update's state (investigating, identified,
        /// monitoring, resolved).
        #[arg(long)]
        state: Option<String>,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Post a dated update: `announce update 12 identified "A bad config"`.
    /// A resolved update also resolves and ends it.
    Update {
        /// The announcement.
        id: i64,
        /// investigating, identified, monitoring or resolved.
        state: String,
        /// What is happening.
        body: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Change an announcement: only the options given change.
    Edit {
        /// The announcement.
        id: i64,
        /// New title.
        #[arg(long)]
        title: Option<String>,
        /// New text.
        #[arg(long)]
        body: Option<String>,
        /// New level.
        #[arg(long)]
        level: Option<String>,
        /// New components (repeat for several).
        #[arg(long = "component")]
        components: Vec<String>,
        /// New start.
        #[arg(long)]
        start: Option<String>,
        /// New end (`""` removes it).
        #[arg(long)]
        end: Option<String>,
        /// Pin (true) or unpin (false).
        #[arg(long)]
        pin: Option<bool>,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// End it now (Mark resolved), with an optional closing word.
    Close {
        /// The announcement.
        id: i64,
        /// Closing word, posted as a resolved update.
        #[arg(long)]
        body: Option<String>,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Delete it and its updates.
    Delete {
        /// The announcement.
        id: i64,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Also send announcements to the alert channels: `on` or `off`
    /// (off by default).
    Send {
        /// on or off.
        switch: String,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

/// A path segment, percent-encoded.
fn enc(v: &str) -> String {
    v.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn base(project: &str) -> String {
    format!("/api/projects/{}/announcements", enc(project))
}

/// Run `announce …`.
pub(crate) fn run(c: &Client, cmd: &AnnounceCmd) -> Result<bool, String> {
    let (status, v) = match cmd {
        AnnounceCmd::List { project, days } => {
            let q = days.map_or_else(String::new, |d| format!("?days={d}"));
            c.call("GET", &format!("{}{q}", base(project)), None)?
        }
        AnnounceCmd::Post {
            title,
            body,
            level,
            components,
            start,
            end,
            pin,
            incident,
            state,
            project,
        } => c.call(
            "POST",
            &base(project),
            Some(Body::Json(json!({
                "title": title, "body": body, "level": level, "components": components,
                "start": start, "end": end, "pinned": pin, "incident": incident, "state": state,
            }))),
        )?,
        AnnounceCmd::Update {
            id,
            state,
            body,
            project,
        } => c.call(
            "POST",
            &format!("{}/{id}/updates", base(project)),
            Some(Body::Json(json!({ "state": state, "body": body }))),
        )?,
        AnnounceCmd::Edit {
            id,
            title,
            body,
            level,
            components,
            start,
            end,
            pin,
            project,
        } => {
            let mut m = Map::new();
            for (k, v) in [
                ("title", title),
                ("body", body),
                ("level", level),
                ("start", start),
                ("end", end),
            ] {
                if let Some(v) = v {
                    m.insert(k.into(), json!(v));
                }
            }
            if !components.is_empty() {
                m.insert("components".into(), json!(components));
            }
            if let Some(p) = pin {
                m.insert("pinned".into(), json!(p));
            }
            if m.is_empty() {
                return Err("give something to change (--title, --body, --level…)".into());
            }
            c.call(
                "PATCH",
                &format!("{}/{id}", base(project)),
                Some(Body::Json(Value::Object(m))),
            )?
        }
        AnnounceCmd::Close { id, body, project } => c.call(
            "POST",
            &format!("{}/{id}/close", base(project)),
            Some(Body::Json(json!({ "body": body }))),
        )?,
        AnnounceCmd::Delete { id, project } => {
            c.call("DELETE", &format!("{}/{id}", base(project)), None)?
        }
        AnnounceCmd::Send { switch, project } => {
            let send = match switch.as_str() {
                "on" => true,
                "off" => false,
                _ => return Err("send on or send off".into()),
            };
            c.call(
                "PUT",
                &format!("{}/send", base(project)),
                Some(Body::Json(json!({ "send": send }))),
            )?
        }
    };
    out(&v);
    Ok((200..300).contains(&status))
}
