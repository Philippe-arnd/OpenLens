//! `openlens-cli maintenance` (#563, D-075): list, schedule, extend, end
//! or cancel planned maintenance windows, so a CI step can open one right
//! before a deploy and end it after.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::{Body, Client, out};

#[derive(Subcommand)]
pub(crate) enum MaintenanceCmd {
    /// Planned and running windows, and those ended within 30 days.
    List {
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Declare a window: its incidents are not announced and its time is
    /// not counted against availability. Prints the window (and its id).
    Schedule {
        /// What is done (shown with the window).
        #[arg(long)]
        text: String,
        /// Start: now (the default), RFC 3339, or `YYYY-MM-DD HH:MM` in
        /// the project's zone.
        #[arg(long)]
        start: Option<String>,
        /// End, like the start; or give --minutes.
        #[arg(long)]
        end: Option<String>,
        /// Length in minutes (at most 4320).
        #[arg(long)]
        minutes: Option<u32>,
        /// `production` (the default) or an environment; repeat for several.
        #[arg(long = "component")]
        components: Vec<String>,
        /// Announce it once on the alert channels.
        #[arg(long)]
        announce: bool,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Move a planned or running window's end: --minutes more, or --end.
    Extend {
        /// The window's id.
        id: i64,
        /// Minutes added to its end.
        #[arg(long)]
        minutes: Option<u32>,
        /// The new end.
        #[arg(long)]
        end: Option<String>,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// End a running window now (after the deploy).
    End {
        /// The window's id.
        id: i64,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Cancel a window that has not started.
    Cancel {
        /// The window's id.
        id: i64,
        /// Project id.
        #[arg(long, default_value = "default")]
        project: String,
    },
}

/// Run one `maintenance` subcommand: true when the server agreed.
pub(crate) fn run(c: &Client, cmd: &MaintenanceCmd) -> Result<bool, String> {
    let at = |project: &str, tail: &str| format!("/api/projects/{project}/maintenance{tail}");
    let (status, v): (u16, Value) = match cmd {
        MaintenanceCmd::List { project } => c.call("GET", &at(project, ""), None)?,
        MaintenanceCmd::Schedule {
            text,
            start,
            end,
            minutes,
            components,
            announce,
            project,
        } => c.call(
            "POST",
            &at(project, ""),
            Some(Body::Json(json!({
                "text": text, "start": start, "end": end, "minutes": minutes,
                "components": components, "announce": announce,
            }))),
        )?,
        MaintenanceCmd::Extend {
            id,
            minutes,
            end,
            project,
        } => c.call(
            "POST",
            &at(project, &format!("/{id}/extend")),
            Some(Body::Json(json!({ "minutes": minutes, "end": end }))),
        )?,
        MaintenanceCmd::End { id, project } => {
            c.call("POST", &at(project, &format!("/{id}/end")), None)?
        }
        MaintenanceCmd::Cancel { id, project } => {
            c.call("DELETE", &at(project, &format!("/{id}")), None)?
        }
    };
    out(&v);
    Ok((200..300).contains(&status))
}
