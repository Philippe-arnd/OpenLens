//! A board computed in the background (#515, D-063): the page a long date
//! range or past release shows while its job waits or runs, and once it
//! failed or was cancelled. No JavaScript: the server sends a `Refresh`
//! header while the job is not finished, and Cancel is a form.

use super::*;

/// What the page shows of a job.
pub struct JobView<'a> {
    /// The job's id (its page is `/p/{project}/board/jobs/{id}`).
    pub id: &'a str,
    /// The window, said ("From 2026-01-01 to 2026-03-01 (60 days)").
    pub label: &'a str,
    /// `queued`, `running`, `failed` or `cancelled` (a finished job shows
    /// the board instead).
    pub state: &'a str,
    /// Seconds spent computing so far.
    pub elapsed_secs: f64,
    /// The estimate's rough time (D-061), 0 when there is none.
    pub estimate_secs: f64,
    /// Why it failed.
    pub error: Option<&'a str>,
    /// The board page for the same parameters (it asks again).
    pub back: &'a str,
}

/// The page of a board job that is not done.
pub fn board_job(p: &ProjectInfo<'_>, j: &JobView<'_>) -> Markup {
    let cancel = html! {
        form method="post" action={ "/p/" (url_enc(p.id)) "/board/jobs/" (url_enc(j.id)) "/cancel" } {
            button.ghost type="submit" { "Cancel" }
        }
    };
    // Never 100 % before it is done: the estimate is rough.
    let pct = if j.estimate_secs > 0.0 {
        (j.elapsed_secs / j.estimate_secs * 100.0).clamp(0.0, 95.0)
    } else {
        0.0
    };
    layout(
        "Focus board",
        Some(p),
        html! {
            div.heading {
                h1 { "Focus board" }
                p.muted { (j.label) ", against the same length before." }
            }
            section.card aria-label="Background board" {
                @match j.state {
                    "queued" => {
                        h2 { "Waiting for its turn" }
                        p { "Other long boards are being computed. This one starts as soon as one of them ends; this page refreshes itself." }
                        (cancel)
                    }
                    "running" => {
                        h2 { "Computing this period" }
                        progress value=(format!("{pct:.0}")) max="100" aria-label="Time spent against the estimate" { (format!("{pct:.0} %")) }
                        p.muted.small {
                            "Running for " (format!("{:.0}", j.elapsed_secs.floor())) " s"
                            @if j.estimate_secs > 0.0 { " of roughly " (format!("{:.0}", j.estimate_secs.ceil())) " s estimated" }
                            ". This page refreshes itself and shows the board when it is ready; you can leave it and come back within the hour."
                        }
                        (cancel)
                    }
                    "cancelled" => {
                        h2 { "Cancelled" }
                        p { "This board was stopped before it finished; nothing was kept." }
                        p { a.button href=(j.back) { "Back to this period" } }
                    }
                    _ => {
                        h2 { "This board could not be computed" }
                        p.alert role="alert" { (j.error.unwrap_or("It stopped with an error.")) }
                        p { a.button href=(j.back) { "Back to this period" } }
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_job_can_be_cancelled_and_a_finished_one_cannot() {
        let p = ProjectInfo {
            id: "p1",
            name: "P",
            key: "k",
        };
        let mut j = JobView {
            id: "j1",
            label: "From 2026-01-01 to 2026-03-01 (60 days)",
            state: "running",
            elapsed_secs: 30.0,
            estimate_secs: 20.0,
            error: None,
            back: "/p/p1?from=2026-01-01",
        };
        let html = board_job(&p, &j).into_string();
        assert!(
            html.contains(r#"action="/p/p1/board/jobs/j1/cancel""#),
            "{html}"
        );
        assert!(
            !html.contains(r#"value="100""#),
            "past the estimate, still not shown as done: {html}"
        );
        j.state = "cancelled";
        let html = board_job(&p, &j).into_string();
        assert!(!html.contains("/cancel"), "{html}");
        assert!(html.contains(r#"href="/p/p1?from=2026-01-01""#), "{html}");
        assert!(!html.contains("<script"), "no JavaScript");
    }
}
