//! The feature map (`openlens.features.yaml`): parsing, validation, role
//! matching, route matching and the SQL expressions the engine uses to
//! resolve raw events into canonical roles and features.
//!
//! The map is the only project configuration (D-014). An agent writes it
//! while wiring the app (docs/agents/wire.md), so validation errors are
//! written for that agent: each one names the path and says how to fix it.
//!
//! Layer: L2 (see `docs/architecture.md`).

mod resolver;
pub use resolver::Resolver;

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
    /// Optional (#269, D-019): the event an explicit cancel sends
    /// (`subscription_cancelled`). A user who sends it counts as churned at
    /// once, without waiting for the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub churn_event: Option<String>,
    /// Canonical roles, in priority order (first match wins).
    #[serde(default)]
    pub roles: Vec<Role>,
    /// Declared features.
    #[serde(default)]
    pub features: Vec<Feature>,
    /// Optional: the currency revenue is shown in, and fixed rates to
    /// convert others into it (#231). Without it, each currency is shown
    /// on its own and nothing is summed across currencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revenue: Option<RevenueSettings>,
}

/// How revenue is shown (#231).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevenueSettings {
    /// ISO 4217 code of the display currency (`EUR`).
    pub currency: String,
    /// Units of the display currency for one unit of another (`USD: 0.92`).
    #[serde(default)]
    pub rates: std::collections::BTreeMap<String, f64>,
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
    /// Optional group shown on the board and the map (one level, e.g.
    /// `Writing`), #240.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    /// Features a user normally uses before this one (keys), #240: the
    /// adoption ladder and the map health check them against usage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Former keys of this feature (#240): events still tagged with an old
    /// key count for this one, so a rename keeps the history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamed_from: Vec<String>,
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
    if let Some(d) = m.churn_after_days
        && !(1..=365).contains(&d)
    {
        v.push(err(
            "churn_after_days",
            "must be between 1 and 365 (default 14)",
        ));
    }
    if let Some(e) = &m.churn_event
        && (e.trim().is_empty() || e.len() > 200)
    {
        v.push(err("churn_event", "event names are 1-200 characters"));
    }
    if let Some(r) = &m.revenue {
        let code = |c: &str| c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase());
        if !code(&r.currency) {
            v.push(err(
                "revenue.currency",
                "an ISO 4217 code in capitals, like EUR or USD",
            ));
        }
        for (k, rate) in &r.rates {
            if !code(k) || !rate.is_finite() || *rate <= 0.0 {
                v.push(err(
                    format!("revenue.rates.{k}"),
                    "a currency code and a positive rate (display units for one unit of it)",
                ));
            }
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
        if let Some(a) = &f.area
            && (a.trim().is_empty() || a.chars().count() > 60 || a.chars().any(char::is_control))
        {
            v.push(err(
                format!("{p}.area"),
                "an area is a short name (1-60 characters), one level: `Writing`, not `Writing/Editor`",
            ));
        }
        for (j, old) in f.renamed_from.iter().enumerate() {
            if !valid_key(old) {
                v.push(err(
                    format!("{p}.renamed_from[{j}]"),
                    format!("`{old}` is not a valid key"),
                ));
            }
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
    relations(m, &keys, &mut v);
    // Every value below goes into the board's queries (#310): one rule.
    // (Routes have their own, stricter one above: no quote at all.)
    let mut in_sql: Vec<(String, &str)> = Vec::new();
    for (i, r) in m.roles.iter().enumerate() {
        in_sql.push((format!("roles[{i}].key"), &r.key));
        for (j, x) in r.matches.iter().enumerate() {
            in_sql.push((format!("roles[{i}].matches[{j}]"), x));
        }
    }
    for (i, f) in m.features.iter().enumerate() {
        in_sql.push((format!("features[{i}].key"), &f.key));
        for (j, x) in f.renamed_from.iter().enumerate() {
            in_sql.push((format!("features[{i}].renamed_from[{j}]"), x));
        }
        if let Some(x) = &f.success {
            in_sql.push((format!("features[{i}].success"), x));
        }
    }
    for (path, value) in in_sql {
        if openlens_sqlgen::checked(value).is_err() {
            v.push(err(path, "1-200 characters, with no control character"));
        }
    }
    v
}

/// `requires` and `renamed_from` across features: unknown keys, a feature
/// requiring itself, cycles, and a former key still in use (#240).
fn relations(m: &FeatureMap, keys: &std::collections::HashSet<String>, v: &mut Vec<Issue>) {
    let mut former = std::collections::HashMap::new();
    for (i, f) in m.features.iter().enumerate() {
        let p = format!("features[{i}]");
        for (j, r) in f.requires.iter().enumerate() {
            if r == &f.key {
                v.push(err(
                    format!("{p}.requires[{j}]"),
                    "a feature cannot require itself",
                ));
            } else if !keys.contains(r) {
                v.push(err(
                    format!("{p}.requires[{j}]"),
                    format!("no feature `{r}`: list keys of features declared in this map"),
                ));
            }
        }
        for (j, old) in f.renamed_from.iter().enumerate() {
            if keys.contains(old) {
                v.push(err(
                    format!("{p}.renamed_from[{j}]"),
                    format!("`{old}` is still the key of a feature: a former key cannot be in use"),
                ));
            } else if let Some(other) = former.insert(old.clone(), f.key.clone()) {
                v.push(err(
                    format!("{p}.renamed_from[{j}]"),
                    format!("`{old}` is already listed as a former key of `{other}`"),
                ));
            }
        }
    }
    // Cycles: depth-first search over `requires`.
    let idx: std::collections::HashMap<&str, usize> = m
        .features
        .iter()
        .enumerate()
        .map(|(i, f)| (f.key.as_str(), i))
        .collect();
    // 0 = unseen, 1 = on the stack, 2 = done.
    let mut state = vec![0u8; m.features.len()];
    for start in 0..m.features.len() {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some(&(node, next)) = stack.last() {
            let f = &m.features[node];
            if let Some(r) = f.requires.get(next) {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                let Some(&t) = idx.get(r.as_str()) else {
                    continue;
                };
                match state[t] {
                    0 => {
                        state[t] = 1;
                        stack.push((t, 0));
                    }
                    1 if t != node => {
                        v.push(err(
                            format!("features[{t}].requires"),
                            format!(
                                "`requires` makes a loop through `{}` and `{}`: a ladder goes one way",
                                m.features[t].key, f.key
                            ),
                        ));
                    }
                    _ => {}
                }
            } else {
                state[node] = 2;
                stack.pop();
            }
        }
    }
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

    /// Every feature whose routes match this path, in map order (the first
    /// one wins; more than one is an overlap, #240).
    pub fn features_for_path(&self, path: &str) -> Vec<&str> {
        self.features
            .iter()
            .filter(|f| f.entry.routes.iter().any(|r| route_matches(r, path)))
            .map(|f| f.key.as_str())
            .collect()
    }

    /// The feature a key belongs to now: itself, or the feature that lists
    /// it in `renamed_from`.
    pub fn current_key<'a>(&'a self, key: &'a str) -> Option<&'a str> {
        self.features.iter().find_map(|f| {
            (f.key == key || f.renamed_from.iter().any(|o| o == key)).then_some(f.key.as_str())
        })
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
    /// else the feature whose success event it is, else the first feature
    /// whose route matches `path`.
    pub fn feature_sql(&self) -> String {
        let mut s = String::from("CASE");
        for f in &self.features {
            if !f.renamed_from.is_empty() {
                let olds: Vec<String> = f
                    .renamed_from
                    .iter()
                    .map(|o| format!("'{}'", sql_escape(o)))
                    .collect();
                s.push_str(&format!(
                    " WHEN feature IN ({}) THEN '{}'",
                    olds.join(", "),
                    sql_escape(&f.key)
                ));
            }
        }
        s.push_str(" WHEN feature IS NOT NULL THEN feature");
        for f in &self.features {
            if let Some(ev) = &f.success {
                s.push_str(&format!(
                    " WHEN event = '{}' THEN '{}'",
                    sql_escape(ev),
                    sql_escape(&f.key)
                ));
            }
        }
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

/// The value inside a SQL literal: quotes doubled by `openlens_sqlgen`,
/// the one place that does it (#310). `validate` refuses what `checked`
/// would.
fn sql_escape(s: &str) -> String {
    let q = openlens_sqlgen::quote(s);
    q[1..q.len() - 1].to_owned()
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
