//! The feature map (`openlens.features.yaml`): parsing, validation, role
//! matching, route matching and the SQL expressions the engine uses to
//! resolve raw events into canonical roles and features.
//!
//! The map is the only project configuration (D-014). An agent writes it
//! while wiring the app (docs/agents/wire.md), so validation errors are
//! written for that agent: each one names the path and says how to fix it.
//!
//! Layer: L2 (see `docs/architecture.md`).

use serde::{Deserialize, Serialize};

/// Largest accepted map, in bytes.
pub const MAX_BYTES: usize = 256 * 1024;
/// Maximum features in a map.
pub const MAX_FEATURES: usize = 500;
/// Maximum roles in a map.
pub const MAX_ROLES: usize = 50;
/// Default churn window, in days.
pub const DEFAULT_CHURN_DAYS: u32 = 14;

/// A parsed feature map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureMap {
    /// Format version (1).
    #[serde(default = "one")]
    pub version: u32,
    /// Days without activity after which a user counts as churned.
    #[serde(default)]
    pub churn_after_days: Option<u32>,
    /// Canonical roles, in priority order (first match wins).
    #[serde(default)]
    pub roles: Vec<Role>,
    /// Declared features.
    #[serde(default)]
    pub features: Vec<Feature>,
}

fn one() -> u32 {
    1
}

/// A canonical role and the raw values that map to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    /// Canonical key, e.g. `admin`.
    pub key: String,
    /// Raw values: exact (case-insensitive) or with `*` wildcards.
    #[serde(rename = "match")]
    pub matches: Vec<String>,
}

/// A declared feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    /// Stable key, e.g. `export-pdf`.
    pub key: String,
    /// Display name.
    pub name: String,
    /// One line: why the feature exists.
    #[serde(default)]
    pub purpose: Option<String>,
    /// Owner (team or person).
    #[serde(default)]
    pub owner: Option<String>,
    /// Where the feature starts.
    #[serde(default)]
    pub entry: Entry,
    /// Expected steps inside the feature (event names), optional.
    #[serde(default)]
    pub steps: Vec<String>,
    /// Event that means "the user got value".
    #[serde(default)]
    pub success: Option<String>,
}

/// Entry points of a feature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Route patterns: `/entries/:id/edit`, `/insights`, `/docs/*`.
    #[serde(default)]
    pub routes: Vec<String>,
    /// CSS selectors (informational; the SDK reports `data-feature`).
    #[serde(default)]
    pub selectors: Vec<String>,
    /// Explicit SDK calls (informational).
    #[serde(default)]
    pub calls: Vec<String>,
}

/// A problem found in a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// `error` (map refused) or `warning` (accepted, but fix it).
    pub level: &'static str,
    /// Where, e.g. `features[2].success`.
    pub path: String,
    /// What is wrong and how to fix it.
    pub message: String,
}

/// Result of [`parse`].
#[derive(Debug, Clone)]
pub struct Parsed {
    /// The map, when there is no error.
    pub map: Option<FeatureMap>,
    /// Errors and warnings.
    pub issues: Vec<Issue>,
}

fn err(path: impl Into<String>, message: impl Into<String>) -> Issue {
    Issue {
        level: "error",
        path: path.into(),
        message: message.into(),
    }
}
fn warn(path: impl Into<String>, message: impl Into<String>) -> Issue {
    Issue {
        level: "warning",
        path: path.into(),
        message: message.into(),
    }
}

/// Is this a valid key: 1–64 chars of `[a-z0-9-_]`, starting with a letter or digit?
pub fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && k.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
}

/// Parse and validate a map. Never panics on any input.
pub fn parse(src: &str) -> Parsed {
    if src.len() > MAX_BYTES {
        return Parsed {
            map: None,
            issues: vec![err(
                "",
                format!(
                    "the map is larger than {MAX_BYTES} bytes; keep it to the features a PM cares about"
                ),
            )],
        };
    }
    let map: FeatureMap = match serde_norway::from_str(src) {
        Ok(m) => m,
        Err(e) => {
            return Parsed {
                map: None,
                issues: vec![err(
                    "",
                    format!("not a valid feature map: {e}. See /agents.md for the format"),
                )],
            };
        }
    };
    let issues = validate(&map);
    let ok = !issues.iter().any(|i| i.level == "error");
    Parsed {
        map: ok.then_some(map),
        issues,
    }
}

/// Validate a map already deserialized.
pub fn validate(m: &FeatureMap) -> Vec<Issue> {
    let mut v = Vec::new();
    if m.version != 1 {
        v.push(err("version", "only version 1 exists; set `version: 1`"));
    }
    if let Some(d) = m.churn_after_days {
        if !(1..=365).contains(&d) {
            v.push(err(
                "churn_after_days",
                "must be between 1 and 365 (default 14)",
            ));
        }
    }
    if m.roles.len() > MAX_ROLES {
        v.push(err(
            "roles",
            format!("at most {MAX_ROLES} roles; group raw values under fewer canonical roles"),
        ));
    }
    if m.features.len() > MAX_FEATURES {
        v.push(err("features", format!("at most {MAX_FEATURES} features")));
    }
    if m.roles.is_empty() {
        v.push(warn("roles", "no roles declared: every KPI is split by role, so list the app's roles and the raw values that map to them"));
    }
    let mut seen = std::collections::HashSet::new();
    for (i, r) in m.roles.iter().enumerate() {
        let p = format!("roles[{i}]");
        if !valid_key(&r.key) {
            v.push(err(
                format!("{p}.key"),
                format!(
                    "`{}` is not a valid key: use lowercase letters, digits, `-` or `_`",
                    r.key
                ),
            ));
        }
        if !seen.insert(r.key.clone()) {
            v.push(err(
                format!("{p}.key"),
                format!("role `{}` is declared twice", r.key),
            ));
        }
        if r.matches.is_empty() {
            v.push(err(
                format!("{p}.match"),
                "list at least one raw value sent by identify(), e.g. [\"owner\", \"admin\"]",
            ));
        }
        for (j, mv) in r.matches.iter().enumerate() {
            if mv.trim().is_empty() || mv.len() > 200 {
                v.push(err(
                    format!("{p}.match[{j}]"),
                    "values must be 1-200 characters",
                ));
            }
        }
    }
    let mut keys = std::collections::HashSet::new();
    for (i, f) in m.features.iter().enumerate() {
        let p = format!("features[{i}]");
        if !valid_key(&f.key) {
            v.push(err(format!("{p}.key"), format!("`{}` is not a valid key: use lowercase letters, digits, `-` or `_` (the same value as data-feature)", f.key)));
        }
        if !keys.insert(f.key.clone()) {
            v.push(err(
                format!("{p}.key"),
                format!("feature `{}` is declared twice", f.key),
            ));
        }
        if f.name.trim().is_empty() || f.name.len() > 120 {
            v.push(err(
                format!("{p}.name"),
                "give the feature a human name (1-120 characters)",
            ));
        }
        match &f.success {
            None => v.push(warn(format!("{p}.success"), "no success event: say which event means the user got value (e.g. `entry_saved`), and track it with openlens.track() if the app does not emit it")),
            Some(s) if s.trim().is_empty() || s.len() > 200 => v.push(err(format!("{p}.success"), "event names are 1-200 characters")),
            _ => {}
        }
        if f.entry.routes.is_empty() && f.entry.selectors.is_empty() && f.entry.calls.is_empty() {
            v.push(warn(format!("{p}.entry"), format!("no entry point: add a route, or put data-feature=\"{}\" on the feature's main control", f.key)));
        }
        for (j, r) in f.entry.routes.iter().enumerate() {
            if !r.starts_with('/')
                || r.len() > 300
                || r.chars()
                    .any(|c| c.is_whitespace() || c == '\'' || c == '\\')
            {
                v.push(err(format!("{p}.entry.routes[{j}]"), "routes start with `/`, use `:param` or `*` for variable parts, and contain no spaces or quotes"));
            }
        }
    }
    v
}

/// Glob match with `*`, case-insensitive.
fn glob(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = value.to_lowercase().chars().collect();
    let (mut pi, mut si, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while si < s.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == s[si] {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

impl FeatureMap {
    /// Canonical role for a raw value (first matching role wins).
    pub fn resolve_role(&self, raw: &str) -> Option<&str> {
        self.roles
            .iter()
            .find(|r| r.matches.iter().any(|m| glob(m.trim(), raw.trim())))
            .map(|r| r.key.as_str())
    }

    /// Feature whose route matches this path.
    pub fn feature_for_path(&self, path: &str) -> Option<&str> {
        self.features
            .iter()
            .find(|f| f.entry.routes.iter().any(|r| route_matches(r, path)))
            .map(|f| f.key.as_str())
    }

    /// Churn window in days.
    pub fn churn_days(&self) -> u32 {
        self.churn_after_days.unwrap_or(DEFAULT_CHURN_DAYS)
    }

    /// SQL expression mapping the raw `role` column to the canonical role:
    /// `unmatched:<raw>` for values no rule covers, NULL when unidentified.
    pub fn role_sql(&self) -> String {
        if self.roles.is_empty() {
            return "role".into();
        }
        let mut s = String::from("CASE WHEN role IS NULL THEN NULL");
        for r in &self.roles {
            for m in &r.matches {
                let pat = sql_escape(&m.trim().to_lowercase());
                if pat.contains('*') {
                    let like = pat
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                        .replace('*', "%");
                    s.push_str(&format!(
                        " WHEN lower(role) LIKE '{like}' THEN '{}'",
                        sql_escape(&r.key)
                    ));
                } else {
                    s.push_str(&format!(
                        " WHEN lower(role) = '{pat}' THEN '{}'",
                        sql_escape(&r.key)
                    ));
                }
            }
        }
        s.push_str(" ELSE concat('unmatched:', role) END");
        s
    }

    /// SQL expression giving the feature of an event: its `feature` column,
    /// else the first feature whose route matches `path`.
    pub fn feature_sql(&self) -> String {
        let mut s = String::from("CASE WHEN feature IS NOT NULL THEN feature");
        for f in &self.features {
            for r in &f.entry.routes {
                s.push_str(&format!(
                    " WHEN regexp_like(path, '{}') THEN '{}'",
                    sql_escape(&route_regex(r)),
                    sql_escape(&f.key)
                ));
            }
        }
        s.push_str(" ELSE NULL END");
        s
    }
}

fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Turn a route pattern into an anchored regex: `:param` matches one
/// segment, `*` matches the rest.
pub fn route_regex(route: &str) -> String {
    let mut out = String::from("^");
    for (i, seg) in route.trim_end_matches('/').split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        if seg == "*" {
            out.push_str(".*");
        } else if seg.starts_with(':') {
            out.push_str("[^/]+");
        } else {
            for c in seg.chars() {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '~' {
                    out.push(c);
                } else {
                    out.push('\\');
                    out.push(c);
                }
            }
        }
    }
    if out == "^" {
        out.push('/');
    }
    out.push_str("/?$");
    out
}

fn route_matches(route: &str, path: &str) -> bool {
    let r: Vec<&str> = route.trim_end_matches('/').split('/').collect();
    let p: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    for (i, seg) in r.iter().enumerate() {
        if *seg == "*" {
            return true;
        }
        match p.get(i) {
            Some(ps) if seg.starts_with(':') && !ps.is_empty() => {}
            Some(ps) if ps == seg => {}
            _ => return false,
        }
    }
    r.len() == p.len()
}
