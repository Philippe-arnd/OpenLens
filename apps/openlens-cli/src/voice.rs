//! `openlens-cli feedback` (#718, D-116): what users said, per rating and
//! feature, with the latest texts (`feedback.list` through the REST API).

use crate::{Client, out};

/// A query value, percent-encoded.
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

/// `GET /api/projects/{project}/feedback`, printed.
pub(crate) fn feedback(
    c: &Client,
    project: &str,
    feature: Option<&str>,
    days: Option<i64>,
) -> Result<bool, String> {
    let mut q = Vec::new();
    if let Some(f) = feature {
        q.push(format!("feature={}", enc(f)));
    }
    if let Some(d) = days {
        q.push(format!("days={d}"));
    }
    let (status, v) = c.call(
        "GET",
        &format!("/api/projects/{}/feedback?{}", enc(project), q.join("&")),
        None,
    )?;
    out(&v);
    Ok(status == 200)
}
