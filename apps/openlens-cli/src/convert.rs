//! Import any per-event export (#233): CSV or JSON lines with their own
//! column names, mapped onto OpenLens events. `--map` names the columns
//! that hold the event, the user, the time and the page; every other
//! column becomes a property. Imported events carry `$backfill: true`.

use serde_json::{Map, Value, json};

/// Which source column feeds each OpenLens field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub event: String,
    pub distinct_id: String,
    pub timestamp: String,
    pub url: Option<String>,
    pub referrer: Option<String>,
    pub session_id: Option<String>,
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            event: "event".into(),
            distinct_id: "distinct_id".into(),
            timestamp: "timestamp".into(),
            url: None,
            referrer: None,
            session_id: None,
        }
    }
}

/// Parse `event=name,distinct_id=user,timestamp=time,url=page,...`.
pub fn mapping(spec: &str) -> Result<Mapping, String> {
    let mut m = Mapping::default();
    for pair in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| format!("`{pair}`: use field=column"))?;
        let v = v.trim().to_owned();
        match k.trim() {
            "event" => m.event = v,
            "distinct_id" | "user" => m.distinct_id = v,
            "timestamp" | "time" => m.timestamp = v,
            "url" => m.url = Some(v),
            "referrer" => m.referrer = Some(v),
            "session_id" | "session" => m.session_id = Some(v),
            other => {
                return Err(format!(
                    "`{other}`: map event, distinct_id, timestamp, url, referrer or session_id"
                ));
            }
        }
    }
    Ok(m)
}

/// Split CSV text into records (RFC 4180: quotes, doubled quotes,
/// newlines inside quotes).
pub fn csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if cell.is_empty() => quoted = true,
            (',', false) => row.push(std::mem::take(&mut cell)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            (c, _) => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// Turn one source record into an OpenLens import line.
fn event(m: &Mapping, rec: &Map<String, Value>) -> Result<String, String> {
    let s = |k: &str| match rec.get(k) {
        Some(Value::String(v)) => Some(v.clone()),
        Some(Value::Null) | None => None,
        Some(v) => Some(v.to_string()),
    };
    let ev = s(&m.event)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("no `{}`", m.event))?;
    let id = s(&m.distinct_id)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("no `{}`", m.distinct_id))?;
    let ts = s(&m.timestamp)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("no `{}`", m.timestamp))?;
    // Unix seconds or milliseconds are accepted too.
    let ts = match ts.parse::<i64>() {
        Ok(n) => {
            let secs = if n > 100_000_000_000 { n / 1000 } else { n };
            chrono::DateTime::from_timestamp(secs, 0).map_or(ts, |d| d.to_rfc3339())
        }
        Err(_) => ts,
    };
    let mut props = Map::new();
    let used: Vec<&str> = [
        Some(m.event.as_str()),
        Some(m.distinct_id.as_str()),
        Some(m.timestamp.as_str()),
        m.url.as_deref(),
        m.referrer.as_deref(),
        m.session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    for (k, v) in rec {
        if !used.contains(&k.as_str()) && !v.is_null() && v != "" {
            props.insert(k.clone(), v.clone());
        }
    }
    if let Some(u) = m.url.as_ref().and_then(|k| s(k)) {
        let path = u
            .split("://")
            .nth(1)
            .and_then(|r| r.find('/').map(|i| r[i..].to_owned()))
            .unwrap_or_else(|| {
                if u.starts_with('/') {
                    u.clone()
                } else {
                    "/".into()
                }
            });
        props.insert("$current_url".into(), Value::String(u));
        props.insert(
            "$pathname".into(),
            Value::String(path.split(['?', '#']).next().unwrap_or("/").to_owned()),
        );
    }
    if let Some(r) = m
        .referrer
        .as_ref()
        .and_then(|k| s(k))
        .filter(|r| !r.is_empty())
    {
        props.insert("$referrer".into(), Value::String(r));
    }
    let mut line = json!({ "event": ev, "distinct_id": id, "timestamp": ts, "properties": props });
    if let Some(sid) = m.session_id.as_ref().and_then(|k| s(k)) {
        line["session_id"] = Value::String(sid);
    }
    Ok(line.to_string())
}

/// Convert an export to OpenLens import lines: `(lines, errors)`.
pub fn convert(
    text: &str,
    format: &str,
    m: &Mapping,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    match format {
        "csv" => {
            let rows = csv(text);
            let Some((head, body)) = rows.split_first() else {
                return Ok((lines, errors));
            };
            let head: Vec<String> = head
                .iter()
                .map(|h| h.trim().trim_start_matches('\u{feff}').to_owned())
                .collect();
            for (i, r) in body.iter().enumerate() {
                let rec: Map<String, Value> = head
                    .iter()
                    .zip(r.iter())
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect();
                match event(m, &rec) {
                    Ok(l) => lines.push(l),
                    Err(e) => errors.push(format!("row {}: {e}", i + 2)),
                }
            }
        }
        "jsonl" => {
            for (i, l) in text
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
            {
                match serde_json::from_str::<Value>(l) {
                    Ok(Value::Object(rec)) => match event(m, &rec) {
                        Ok(line) => lines.push(line),
                        Err(e) => errors.push(format!("line {}: {e}", i + 1)),
                    },
                    _ => errors.push(format!("line {}: not a JSON object", i + 1)),
                }
            }
        }
        other => return Err(format!("`{other}`: the format is csv or jsonl")),
    }
    Ok((lines, errors))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn maps_a_csv_export_onto_events() {
        let text = "\u{feff}event_name,user_id,time,page_url,ref,plan,note\r\n\
                    page_view,u1,1732350000,https://app.example.com/pricing?x=1,https://news.example.org/,pro,\"a, \"\"quoted\"\" note\"\n\
                    signup,u2,2026-11-20T10:00:00Z,/start,,free,\"two\nlines\"\n\
                    ,u3,2026-11-20T10:00:00Z,,,,\n";
        let m = mapping(
            "event=event_name,distinct_id=user_id,timestamp=time,url=page_url,referrer=ref",
        )
        .unwrap();
        let (lines, errors) = convert(text, "csv", &m).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(errors, vec!["row 4: no `event_name`".to_owned()]);
        let a: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(a["event"], "page_view");
        assert_eq!(a["timestamp"], "2024-11-23T08:20:00+00:00");
        assert_eq!(a["properties"]["$pathname"], "/pricing");
        assert_eq!(a["properties"]["$referrer"], "https://news.example.org/");
        assert_eq!(a["properties"]["note"], "a, \"quoted\" note");
        assert_eq!(a["properties"]["plan"], "pro");
        assert!(a["properties"].get("user_id").is_none());
        let b: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(b["properties"]["note"], "two\nlines");
        assert_eq!(b["properties"]["$pathname"], "/start");
    }

    #[test]
    fn maps_json_lines_with_their_own_names() {
        let text = "{\"name\":\"click\",\"uid\":\"u1\",\"ts\":1732350000123,\"button\":\"buy\"}\nnot json\n";
        let m = mapping("event=name,user=uid,time=ts").unwrap();
        let (lines, errors) = convert(text, "jsonl", &m).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(errors.len(), 1);
        let a: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(a["properties"]["button"], "buy");
        assert_eq!(a["timestamp"], "2024-11-23T08:20:00+00:00");
        assert!(mapping("colour=x").is_err());
        assert!(convert("", "xml", &m).is_err());
    }
}
