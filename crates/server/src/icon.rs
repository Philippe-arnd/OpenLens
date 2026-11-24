//! The project's icon (#214): the app's own favicon, fetched by the server
//! from the app's address, or the project's initial when there is none.
//!
//! The address is the project's `app_url` setting, else the origin most
//! pageviews of the last 7 days came from (no setting needed). Events can
//! be sent by anyone holding the public key, so the fetch only reaches
//! public addresses (checked after DNS, on every redirect), reads at most
//! [`MAX_BYTES`], and keeps the bytes only if they are an image.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::AppState;

/// Largest icon or page read.
const MAX_BYTES: u64 = 128 * 1024;
/// An icon is fetched again after this many days.
const REFRESH_DAYS: i64 = 7;

/// `GET /p/{id}/icon`: the stored favicon, else an SVG with the initial.
/// Starts a refresh in the background when the icon is missing or old.
pub(crate) async fn icon(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let meta = state.meta.clone();
    let pid = id.clone();
    let Ok(Ok(Some(p))) = tokio::task::spawn_blocking(move || meta.project(&pid)).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    refresh_if_due(&state, &p.id);
    let bytes = std::fs::read(path(&state, &p.id)).ok();
    match bytes.as_deref().and_then(|b| sniff(b).map(|m| (m, b))) {
        Some((mime, b)) => image(mime, b.to_vec()),
        None => image("image/svg+xml", initial_svg(&p.name).into_bytes()),
    }
}

fn image(mime: &'static str, body: Vec<u8>) -> Response {
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    // An SVG opened on its own must not run anything.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    r
}

fn path(state: &AppState, id: &str) -> std::path::PathBuf {
    state.data_dir.join("icons").join(format!("{id}.img"))
}

/// Forget the stored icon (a new app address, or a deleted project).
pub(crate) fn forget(state: &AppState, id: &str) {
    let _ = std::fs::remove_file(path(state, id));
    let _ = state.meta.set_project_setting(id, "icon_checked", "");
}

/// The kind of image these bytes are, if they are one.
pub(crate) fn sniff(b: &[u8]) -> Option<&'static str> {
    let head = b.get(..12)?;
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else if head.starts_with(b"GIF8") {
        Some("image/gif")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        let text = std::str::from_utf8(b.get(..b.len().min(512))?).ok()?;
        let t = text.trim_start_matches('\u{feff}').trim_start();
        (t.starts_with("<svg") || (t.starts_with("<?xml") && text.contains("<svg")))
            .then_some("image/svg+xml")
    }
}

/// The project's initial on the brand gradient (readable in both themes).
pub(crate) fn initial_svg(name: &str) -> String {
    let c = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().to_string());
    let c = match c.as_str() {
        "<" | ">" | "&" => "?".to_owned(),
        _ => c,
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#7c5cff"/><stop offset="1" stop-color="#2f9bd8"/></linearGradient></defs><rect width="24" height="24" rx="7" fill="url(#g)"/><text x="12" y="16.2" text-anchor="middle" font-family="system-ui,sans-serif" font-size="12" font-weight="700" fill="#fff">{c}</text></svg>"##
    )
}

fn refresh_if_due(state: &AppState, id: &str) {
    let checked = state
        .meta
        .project_setting(id, "icon_checked")
        .ok()
        .flatten()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok());
    if checked
        .is_some_and(|t| chrono::Utc::now() - t.to_utc() < chrono::Duration::days(REFRESH_DAYS))
    {
        return;
    }
    // One fetch per icon at a time.
    let first = state
        .caches
        .icons_in_flight
        .update(id.to_owned(), |was| (Some(()), was.is_none()));
    if first != Some(true) {
        return;
    }
    let state = state.clone();
    let id = id.to_owned();
    tokio::spawn(async move {
        if let Some(origin) = app_origin(&state, &id).await {
            let got = tokio::task::spawn_blocking(move || fetch_icon(&origin))
                .await
                .ok()
                .flatten();
            if let Some(bytes) = got {
                let p = path(&state, &id);
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = p.with_extension("tmp");
                if std::fs::write(&tmp, &bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &p);
                }
            }
        }
        // Checked even when nothing was found: try again in a week.
        let _ =
            state
                .meta
                .set_project_setting(&id, "icon_checked", &chrono::Utc::now().to_rfc3339());
        state.caches.icons_in_flight.remove(&id);
    });
}

/// The app's origin: the `app_url` setting, else where most pageviews of
/// the last 7 days came from.
pub(crate) async fn app_origin(state: &AppState, id: &str) -> Option<String> {
    if let Some(u) = state.meta.project_setting(id, "app_url").ok().flatten() {
        return origin(&u);
    }
    detected_origin(state, id).await
}

/// Where most pageviews of the last 7 days came from.
pub(crate) async fn detected_origin(state: &AppState, id: &str) -> Option<String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d");
    let sql = format!(
        "SELECT split_part(url, '/', 1) || '//' || split_part(url, '/', 3) AS o, count(*) AS n \
         FROM events WHERE date >= '{since}' AND event = '$pageview' AND url LIKE 'http%' \
         GROUP BY 1 ORDER BY n DESC LIMIT 1"
    );
    let r = state.query.sql(id, &sql).await.ok()?;
    let o = r.rows.first()?.get("o")?.as_str()?;
    origin(o)
}

/// `scheme://host[:port]` of an http(s) URL, or None.
pub(crate) fn origin(u: &str) -> Option<String> {
    let u = u.trim();
    let (scheme, rest) = u.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty()
        || host.contains('@')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    {
        return None;
    }
    Some(format!("{scheme}://{}", host.to_ascii_lowercase()))
}

/// Only public addresses: no loopback, private, link-local, shared or
/// unique-local ranges (the server's own network stays out of reach).
pub(crate) fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_unspecified()
                || v.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || o[0] >= 224)
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let s = v.segments();
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

#[derive(Debug, Default)]
struct PublicOnly(DefaultResolver);

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = self.0.resolve(uri, config, timeout)?;
        let mut out = self.empty();
        for a in all.iter().filter(|a: &&SocketAddr| public(a.ip())) {
            out.push(*a);
        }
        if out.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(out)
    }
}

fn agent() -> ureq::Agent {
    public_agent("OpenLens icon fetcher", 6, true)
}

/// An HTTP client that only reaches public addresses (checked after DNS,
/// on every redirect): for the icon fetcher, the uptime probe and alert
/// webhooks, whose addresses come from settings or events.
pub(crate) fn public_agent(ua: &str, secs: u64, status_as_error: bool) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(secs)))
        .max_redirects(3)
        .user_agent(ua)
        .http_status_as_error(status_as_error)
        .build();
    ureq::Agent::with_parts(config, DefaultConnector::new(), PublicOnly::default())
}

fn get(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let mut r = agent.get(url).call().ok()?;
    r.body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .ok()
}

/// The icon the app's home page declares, else `/favicon.ico`.
fn fetch_icon(origin: &str) -> Option<Vec<u8>> {
    let a = agent();
    let declared = get(&a, &format!("{origin}/"))
        .and_then(|html| icon_href(&String::from_utf8_lossy(&html)))
        .and_then(|href| absolute(origin, &href));
    for url in declared
        .into_iter()
        .chain([format!("{origin}/favicon.ico")])
    {
        if let Some(b) = get(&a, &url)
            && sniff(&b).is_some()
        {
            return Some(b);
        }
    }
    None
}

/// The `href` of the first `<link rel="icon">` (or `shortcut icon`,
/// `apple-touch-icon`) in a page, preferring `icon`.
pub(crate) fn icon_href(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut best: Option<(u8, String)> = None;
    let mut at = 0;
    while let Some(i) = lower[at..].find("<link") {
        let start = at + i;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &html[start..end];
        let ltag = &lower[start..end];
        at = end;
        let Some(rel) = attr(ltag, tag, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let rank = if rel.split_whitespace().any(|w| w == "icon") {
            0
        } else if rel.contains("apple-touch-icon") {
            1
        } else {
            continue;
        };
        if let Some(href) = attr(ltag, tag, "href")
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
        {
            best = Some((rank, href));
        }
    }
    best.map(|(_, h)| h)
}

fn attr(ltag: &str, tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = ltag[from..].find(name) {
        let p = from + i;
        from = p + name.len();
        let before = ltag[..p].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = ltag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let off = ltag.len() - rest.len();
        let raw = tag[off..].trim_start();
        let v = match raw.chars().next()? {
            q @ ('"' | '\'') => raw[1..].split(q).next()?,
            _ => raw.split(|c: char| c.is_whitespace() || c == '>').next()?,
        };
        return Some(v.replace("&amp;", "&"));
    }
    None
}

/// A link of the page at `origin`, made absolute (http(s) only).
pub(crate) fn absolute(origin: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.starts_with("data:") || href.is_empty() {
        return None;
    }
    if let Some(rest) = href.strip_prefix("//") {
        let scheme = origin.split("://").next()?;
        return Some(format!("{scheme}://{rest}"));
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_owned());
    }
    if href.contains(':') && !href.starts_with('/') {
        return None;
    }
    let path = href.trim_start_matches("./");
    Some(if path.starts_with('/') {
        format!("{origin}{path}")
    } else {
        format!("{origin}/{path}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_declared_icon() {
        let html = r#"<head><link rel="stylesheet" href="/a.css"><link rel="apple-touch-icon" href="/t.png"><LINK REL="icon" type="image/svg+xml" href='/favicon.svg'></head>"#;
        assert_eq!(icon_href(html).as_deref(), Some("/favicon.svg"));
        assert_eq!(
            icon_href(r#"<link href="i.png" rel="shortcut icon">"#).as_deref(),
            Some("i.png")
        );
        assert_eq!(icon_href("<link rel=preload href=x>"), None);
        assert_eq!(
            absolute("https://a.example", "i.png").as_deref(),
            Some("https://a.example/i.png")
        );
        assert_eq!(
            absolute("https://a.example", "//cdn.example/i.png").as_deref(),
            Some("https://cdn.example/i.png")
        );
        assert_eq!(absolute("https://a.example", "javascript:x"), None);
        assert_eq!(
            absolute("https://a.example", "data:image/png;base64,x"),
            None
        );
    }

    #[test]
    fn only_public_addresses_and_web_origins() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public(bad.parse().unwrap()), "{bad}");
        }
        for ok in ["1.1.1.1", "2606:4700::1111"] {
            assert!(public(ok.parse().unwrap()), "{ok}");
        }
        assert_eq!(
            origin("https://App.example:8443/x?y").as_deref(),
            Some("https://app.example:8443")
        );
        assert_eq!(origin("file:///etc/passwd"), None);
        assert_eq!(origin("https://user@evil.example/"), None);
    }

    #[test]
    fn keeps_only_images() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n0000"), Some("image/png"));
        assert_eq!(sniff(b"\0\0\x01\0\x01\0 0000000"), Some("image/x-icon"));
        assert_eq!(sniff(b"<?xml version='1.0'?><svg/>"), Some("image/svg+xml"));
        assert_eq!(sniff(b"<!doctype html><html>"), None);
        assert!(initial_svg("journal").contains(">J</text>"));
        assert!(initial_svg("<x").contains(">X</text>"));
    }
}
