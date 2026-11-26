//! Go to… (#541): what a typed name means in a project's data, for the
//! sidebar field. The section names are the dashboard's own routing data
//! and are matched by the UI; this command only answers for the project's
//! features and people.

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};

/// The longest name Go to… looks up, in bytes. Longer input is refused:
/// no feature key, feature name or person id we keep is that long.
pub const MAX_QUERY: usize = 200;

/// At most this many partial feature matches are returned.
const MAX_PARTIAL: usize = 20;

/// The input of `goto.lookup`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LookupInput {
    /// Project id.
    pub project: String,
    /// What the person typed.
    pub q: String,
}

/// A feature of the map: its key and display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureRef {
    /// Key (the current one, never a former key).
    pub key: String,
    /// Display name.
    pub name: String,
}

/// What a name matches in the project's data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Matches {
    /// Features whose key, former key or name is the name, ignoring case
    /// and surrounding spaces.
    pub features: Vec<FeatureRef>,
    /// Other features whose key or name contains it (at most 20).
    pub partial: Vec<FeatureRef>,
    /// A person with exactly this id (ids are case-sensitive; only the
    /// surrounding spaces are trimmed).
    pub person: Option<String>,
}

/// `goto.lookup`: the features and the person a name points to.
pub struct Lookup;

impl Command for Lookup {
    const NAME: &'static str = "goto.lookup";
    const ACCESS: Access = Access::Read;
    type Input = LookupInput;
    type Output = Matches;

    async fn run<H: Host>(host: &H, _: &Caller, input: LookupInput) -> Result<Matches, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        if input.q.len() > MAX_QUERY {
            return Err(Error::Invalid(format!(
                "Type at most {MAX_QUERY} characters."
            )));
        }
        let q = input.q.trim();
        let mut out = Matches::default();
        if q.is_empty() {
            return Ok(out);
        }
        if let Some((m, _)) = host.feature_map(&p.id).await {
            out = matches(&m, q);
        }
        out.person = host
            .meta()
            .person(&p.id, q)
            .ok()
            .flatten()
            .map(|person| person.distinct_id);
        Ok(out)
    }
}

/// The features of `m` that `q` (trimmed, not empty) names exactly, then
/// the ones it is part of.
fn matches(m: &openlens_featuremap::FeatureMap, q: &str) -> Matches {
    let lower = q.to_lowercase();
    let mut out = Matches::default();
    for f in &m.features {
        let r = FeatureRef {
            key: f.key.clone(),
            name: f.name.clone(),
        };
        let exact = f.key.eq_ignore_ascii_case(q)
            || f.name.trim().to_lowercase() == lower
            || f.renamed_from.iter().any(|o| o.eq_ignore_ascii_case(q));
        if exact {
            out.features.push(r);
        } else if out.partial.len() < MAX_PARTIAL
            && (f.key.to_lowercase().contains(&lower) || f.name.to_lowercase().contains(&lower))
        {
            out.partial.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> openlens_featuremap::FeatureMap {
        openlens_featuremap::parse(
            "features:\n  - key: export-pdf\n    name: Export to PDF\n    renamed_from: [pdf]\n    entry: { routes: ['/export'] }\n  - key: export-csv\n    name: CSV export\n    entry: { routes: ['/csv'] }\n",
        )
        .map
        .unwrap_or_default()
    }

    #[test]
    fn a_name_a_key_or_a_former_key_is_exact_and_a_part_is_partial() {
        let m = map();
        for q in ["export to pdf", "EXPORT-PDF", "pdf"] {
            let r = matches(&m, q);
            assert_eq!(r.features.len(), 1, "{q}");
            assert_eq!(r.features[0].key, "export-pdf", "{q}");
        }
        let r = matches(&m, "export");
        assert!(r.features.is_empty());
        assert!(r.partial.len() > 1, "both features contain it");
    }
}
