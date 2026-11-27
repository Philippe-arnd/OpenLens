//! `cargo xtask demo-site <dir> [--bin PATH]`: the online demo (#304).
//!
//! Writes the `cargo xtask demo` data set into a temporary data dir, starts
//! the real server on a free local port, signs in with the admin token like
//! a user does (`POST /login`), and crawls the default project from its
//! board: Features, Errors, Churn and the pages their cards link to, within
//! a page cap and a depth cap. Each page is saved as a static file with
//! relative links, so the folder works anywhere (`/demo/` on the project
//! site). Forms are disabled or replaced with a note, scripts are dropped,
//! and nothing in the output calls an OpenLens server. Session replays,
//! settings and exports are not crawled: their links open a note page.
//! The server is stopped and the data dir removed at the end, error or not.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROJECT: &str = "default";
/// At most this many HTML pages.
const MAX_PAGES: usize = 300;
/// Clicks away from the board.
const MAX_DEPTH: usize = 3;
/// A response larger than this is not part of the demo.
const MAX_BODY: u64 = 16 * 1024 * 1024;
/// Where links to pages outside the demo go.
const NOTE: &str = "not-in-demo.html";
/// What the server's own address becomes in text (e.g. a snippet).
const PLACEHOLDER_ORIGIN: &str = "https://your-openlens.example";
const REPO: &str = "https://github.com/Philippe-arnd/OpenLens";

/// Paths under the project that are not crawled: replays (a player and
/// its JSON), writes behind a GET, exports, settings (keys and tokens).
const SKIP: &[&str] = &[
    "/settings",
    "/replays/",
    "/watch",
    "/export",
    "/ask/export.md",
    "/tickets/new",
    "/heatmap/overlay",
    "/heatmap.json",
    "/icon",
];

pub(crate) fn demo_site(args: &[String]) -> Result<(), String> {
    let mut out = None;
    let mut bin = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--bin" => bin = Some(PathBuf::from(it.next().ok_or("--bin needs a value")?)),
            _ if out.is_none() && !a.starts_with('-') => out = Some(PathBuf::from(a)),
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    let out = out.ok_or("usage: cargo xtask demo-site <dir> [--bin PATH]")?;
    if out.exists()
        && std::fs::read_dir(&out)
            .map_err(|e| format!("{}: {e}", out.display()))?
            .next()
            .is_some()
    {
        return Err(format!("{} is not empty", out.display()));
    }
    let bin = match bin {
        Some(b) => b,
        None => {
            crate::run("cargo", &["build", "-p", "openlens"])?;
            let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
            Path::new(&target).join("debug").join("openlens")
        }
    };
    let server = Server::start(&bin)?;
    let site = crawl(&server)?;
    drop(server);
    write_site(&out, &site)?;
    eprintln!(
        "xtask: demo site in {} ({} pages, {} assets)",
        out.display(),
        site.pages.len(),
        site.assets.len()
    );
    Ok(())
}

/// A temporary data dir, removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The running server. Dropping it stops the process, then removes its
/// data (`_data` drops after `drop` returns).
struct Server {
    child: Child,
    /// Held for its `Drop` only.
    _data: TempDir,
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Server {
    fn start(bin: &Path) -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let data = TempDir(std::env::temp_dir().join(format!(
            "openlens-demo-site-{}-{nanos:x}",
            std::process::id()
        )));
        std::fs::create_dir_all(&data.0).map_err(|e| format!("{}: {e}", data.0.display()))?;
        crate::write_demo(&data.0.join("data"))?;
        let addr = free_addr()?;
        let token = format!("demo-site-{nanos:x}");
        let log_path = data.0.join("server.log");
        let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        let child = Command::new(bin)
            .env("OPENLENS_DATA_DIR", data.0.join("data"))
            .env("OPENLENS_ADDR", &addr)
            .env("OPENLENS_ADMIN_TOKEN", &token)
            .env("OPENLENS_UPDATE_CHECK", "off")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
        // From here on, `Drop` stops the process whatever happens.
        let server = Self {
            child,
            _data: data,
            base: format!("http://{addr}"),
            token,
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(120)))
                .http_status_as_error(false)
                .max_redirects(0)
                .build()
                .new_agent(),
        };
        let up = Instant::now();
        loop {
            if matches!(server.get("/healthz", None), Ok(r) if r.status == 200) {
                return Ok(server);
            }
            if up.elapsed() > Duration::from_secs(60) {
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                return Err(format!("the server did not start:\n{log}"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn get(&self, path: &str, cookie: Option<&str>) -> Result<Resp, String> {
        let url = format!("{}{path}", self.base);
        let mut req = self.agent.get(&url);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let resp = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        Resp::read(resp)
    }

    /// Sign in with the admin token, as the login form does: the session
    /// cookie to send with every request.
    fn login(&self) -> Result<String, String> {
        let resp = self
            .agent
            .post(format!("{}/login", self.base))
            .header("content-type", "application/x-www-form-urlencoded")
            .send(format!("token={}", self.token).as_bytes())
            .map_err(|e| format!("POST /login: {e}"))?;
        resp.headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .find(|kv| kv.split_once('=').is_some_and(|(_, v)| !v.is_empty()))
            .map(str::to_owned)
            .ok_or_else(|| format!("sign-in failed (HTTP {})", resp.status().as_u16()))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_addr() -> Result<String, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.to_string())
}

struct Resp {
    status: u16,
    content_type: String,
    location: Option<String>,
    body: Vec<u8>,
}

impl Resp {
    fn read(resp: ureq::http::Response<ureq::Body>) -> Result<Self, String> {
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let status = resp.status().as_u16();
        let content_type = header("content-type").unwrap_or_default();
        let location = header("location");
        let body = resp
            .into_body()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            status,
            content_type,
            location,
            body,
        })
    }
}

/// What the crawl brought back, before rewriting.
struct Site {
    /// URL (path and query) → HTML.
    pages: BTreeMap<String, String>,
    /// URL → URL it redirects to.
    aliases: BTreeMap<String, String>,
    /// `/assets/…` URL → bytes.
    assets: BTreeMap<String, Vec<u8>>,
    /// The project icon, with its file name.
    icon: Option<(String, Vec<u8>)>,
    origin: String,
}

fn crawl(server: &Server) -> Result<Site, String> {
    let cookie = server.login()?;
    let root = format!("/p/{PROJECT}");
    let mut site = Site {
        pages: BTreeMap::new(),
        aliases: BTreeMap::new(),
        assets: BTreeMap::new(),
        icon: None,
        origin: server.base.clone(),
    };
    let mut queue = VecDeque::from([(root.clone(), 0)]);
    for p in ["features", "errors", "churn"] {
        queue.push_back((format!("{root}/{p}"), 1));
    }
    let mut seen: BTreeSet<String> = queue.iter().map(|(u, _)| u.clone()).collect();
    // The "not in the demo" page links the unhashed stylesheet (#579).
    let mut asset_urls = BTreeSet::from(["/assets/app.css".to_owned()]);
    while let Some((url, depth)) = queue.pop_front() {
        if site.pages.len() >= MAX_PAGES {
            break;
        }
        let r = server.get(&url, Some(&cookie))?;
        if (300..400).contains(&r.status) {
            if let Some(to) = r.location.and_then(|l| absolute(&l, &url, &site.origin))
                && follow(&to)
            {
                site.aliases.insert(url, to.clone());
                if seen.insert(to.clone()) {
                    queue.push_back((to, depth));
                }
            }
            continue;
        }
        if r.status != 200 || !r.content_type.starts_with("text/html") {
            eprintln!(
                "xtask: skipped {url} (HTTP {}, {})",
                r.status, r.content_type
            );
            continue;
        }
        let html = String::from_utf8_lossy(&r.body).into_owned();
        // A POST form's action is a write, not a page.
        for link in links(&disable_forms(&html), &url, &site.origin) {
            if link.starts_with("/assets/") && !link.ends_with(".js") {
                asset_urls.insert(link);
            } else if depth < MAX_DEPTH && follow(&link) && seen.insert(link.clone()) {
                queue.push_back((link, depth + 1));
            }
        }
        site.pages.insert(url, html);
    }
    for url in asset_urls {
        let r = server.get(&url, Some(&cookie))?;
        if r.status == 200 {
            site.assets.insert(url, r.body);
        }
    }
    let r = server.get(&format!("{root}/icon"), Some(&cookie))?;
    if r.status == 200 {
        let ext = match r.content_type.as_str() {
            t if t.starts_with("image/svg") => "svg",
            t if t.starts_with("image/png") => "png",
            t if t.starts_with("image/x-icon") || t.contains("icon") => "ico",
            t if t.starts_with("image/jpeg") => "jpg",
            _ => "img",
        };
        site.icon = Some((format!("project-icon.{ext}"), r.body));
    }
    Ok(site)
}

fn write_site(out: &Path, site: &Site) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let write = |name: &str, bytes: &[u8]| {
        std::fs::write(out.join(name), bytes).map_err(|e| format!("{name}: {e}"))
    };
    let icon_url = format!("/p/{PROJECT}/icon");
    let root = format!("/p/{PROJECT}");
    let target = |url: &str| -> Option<String> {
        // The brand link (`/`) leads back to the board.
        let url = if url == "/" { root.as_str() } else { url };
        let url = site.aliases.get(url).map_or(url, String::as_str);
        if site.pages.contains_key(url) {
            return Some(file_name(url, PROJECT));
        }
        if site.assets.contains_key(url) {
            return Some(asset_name(url));
        }
        if url == icon_url {
            return site.icon.as_ref().map(|(n, _)| n.clone());
        }
        None
    };
    for (url, html) in &site.pages {
        write(
            &file_name(url, PROJECT),
            rewrite(html, url, &site.origin, &target).as_bytes(),
        )?;
    }
    for (url, bytes) in &site.assets {
        write(&asset_name(url), bytes)?;
    }
    if let Some((name, bytes)) = &site.icon {
        write(name, bytes)?;
    }
    write(NOTE, note_page().as_bytes())
}

/// Is `url` (path and query) a page of the demo project worth crawling?
fn follow(url: &str) -> bool {
    let root = format!("/p/{PROJECT}");
    let Some(rest) = url.strip_prefix(&root) else {
        return false;
    };
    let path = rest.split('?').next().unwrap_or_default();
    (path.is_empty() || path.starts_with('/'))
        // Files (`.json`, `.csv`, `.md`) are downloads, not pages.
        && !path.rsplit('/').next().unwrap_or_default().contains('.')
        && !SKIP.iter().any(|s| path.starts_with(s))
}

/// `href`/`src`/`action` value → absolute path and query, when it points at
/// this server. Fragments are dropped.
fn absolute(value: &str, page: &str, origin: &str) -> Option<String> {
    let v = unescape(value);
    let v = v.split('#').next().unwrap_or_default();
    let v = v.strip_prefix(origin).unwrap_or(v);
    if v.starts_with('/') && !v.starts_with("//") {
        Some(v.to_owned())
    } else if v.starts_with('?') {
        Some(format!("{}{v}", page.split('?').next().unwrap_or_default()))
    } else {
        None
    }
}

/// Every link of a page to this server, as absolute paths.
fn links(html: &str, page: &str, origin: &str) -> Vec<String> {
    let mut found = Vec::new();
    rewrite_attrs(html, |v| {
        if let Some(u) = absolute(v, page, origin) {
            found.push(u);
        }
        v.to_owned()
    });
    found
}

const ATTRS: &[&str] = &[" href=\"", " src=\"", " action=\"", " formaction=\""];

/// Calls `f` on every link attribute value (`href`, `src`, `action`,
/// `formaction`) and puts its answer in place. maud always quotes with `"`
/// and escapes `"` in text, so a match is always markup.
fn rewrite_attrs(html: &str, mut f: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some((at, len)) = ATTRS
        .iter()
        .filter_map(|a| rest.find(a).map(|i| (i, a.len())))
        .min()
    {
        let (head, tail) = rest.split_at(at + len);
        out.push_str(head);
        let Some(end) = tail.find('"') else {
            rest = tail;
            break;
        };
        out.push_str(&f(&tail[..end]));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// One saved page, ready for a static host: links relative, forms off,
/// no script, no refresh, and the server's address nowhere.
fn rewrite(
    html: &str,
    page: &str,
    origin: &str,
    target: &dyn Fn(&str) -> Option<String>,
) -> String {
    let html = disable_forms(html);
    let html = strip_element(&html, "<script", "</script>");
    let html = strip_element(&html, "<meta http-equiv=\"refresh\"", "");
    let html = rewrite_attrs(&html, |v| {
        let fragment = v.find('#').map_or("", |i| &v[i..]);
        if v.starts_with('#') || v.starts_with("data:") {
            return v.to_owned();
        }
        match absolute(v, page, origin) {
            Some(u) => target(&u).map_or_else(|| NOTE.to_owned(), |f| format!("{f}{fragment}")),
            // A link elsewhere on the web stays.
            None if v.contains("://") || v.starts_with("mailto:") => v.to_owned(),
            None => NOTE.to_owned(),
        }
    });
    let html = html.replace(origin, PLACEHOLDER_ORIGIN);
    add_banner(&html)
}

/// POST forms change data: each becomes a short note (sign-out disappears).
/// GET forms (filters) stay visible inside a disabled fieldset.
fn disable_forms(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<form") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let (Some(open_end), Some(close)) = (tail.find('>'), tail.find("</form>")) else {
            rest = tail;
            break;
        };
        let open = &tail[..=open_end];
        let inner = &tail[open_end + 1..close.max(open_end + 1)];
        if open.to_ascii_lowercase().contains("method=\"post\"") {
            if !open.contains("action=\"/logout\"") {
                out.push_str("<p class=\"demo-note\"><small>Read-only demo</small></p>");
            }
        } else {
            out.push_str(open);
            out.push_str("<fieldset disabled style=\"display:contents\">");
            out.push_str(inner);
            out.push_str("</fieldset></form>");
        }
        rest = &tail[close + "</form>".len()..];
    }
    out.push_str(rest);
    out
}

/// Removes every element that starts with `open` up to the end of `close`
/// (or of its opening tag when `close` is empty).
fn strip_element(html: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = if close.is_empty() {
            tail.find('>').map(|i| i + 1)
        } else {
            tail.find(close).map(|i| i + close.len())
        };
        rest = end.map_or("", |e| &tail[e..]);
    }
    out.push_str(rest);
    out
}

fn add_banner(html: &str) -> String {
    let banner = format!(
        "<p class=\"demo-banner\" style=\"margin:0;padding:.5rem 1rem;text-align:center;background:#6d5efc;color:#fff\">Read-only demo on synthetic data. <a style=\"color:#fff\" href=\"{REPO}\">Self-host OpenLens</a> to explore your own app.</p>"
    );
    match html.find("<body") {
        Some(i) => match html[i..].find('>') {
            Some(j) => format!("{}{banner}{}", &html[..=i + j], &html[i + j + 1..]),
            None => html.to_owned(),
        },
        None => html.to_owned(),
    }
}

fn unescape(v: &str) -> String {
    v.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The file a page of `project` is saved as, flat in the output folder so
/// every link is a bare file name. The board is `index.html`; a plain path
/// keeps its words (`/p/x/features/a` → `features.a.html`); anything else
/// (a query string, other characters) gets a readable slug and a hash of
/// the full URL, so two URLs never share a file.
pub(crate) fn file_name(url: &str, project: &str) -> String {
    let root = format!("/p/{project}");
    let rest = url.strip_prefix(&root).unwrap_or(url);
    if rest.is_empty() || rest == "/" {
        return "index.html".to_owned();
    }
    let path = rest.trim_matches('/');
    let plain = !rest.contains('?')
        && !path.contains("//")
        && path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'/');
    if plain && path != "index" {
        return format!("{}.html", path.replace('/', "."));
    }
    let mut slug = String::new();
    for c in rest.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 80 {
            break;
        }
    }
    format!("{}-{:016x}.html", slug.trim_end_matches('-'), fnv(rest))
}

/// `/assets/app.css` → `app.css`.
fn asset_name(url: &str) -> String {
    let name = url.rsplit('/').next().unwrap_or_default();
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !name.starts_with('.');
    if ok {
        format!("asset-{name}")
    } else {
        format!("asset-{:016x}", fnv(url))
    }
}

/// FNV-1a: stable across runs and platforms, unlike `DefaultHasher`.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn note_page() -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\"><title>Not in the demo · OpenLens</title><link rel=\"stylesheet\" href=\"asset-app.css\"></head>\
<body><main style=\"max-width:40rem;margin:3rem auto;padding:0 1rem\"><h1>Not in the demo</h1>\
<p>This page needs a running OpenLens server. The demo is a read-only copy of the board on synthetic data: session replays, settings, exports and anything that changes data are left out.</p>\
<p><a href=\"index.html\">Back to the board</a> · <a href=\"{REPO}\">Self-host OpenLens</a></p></main></body></html>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_target(url: &str) -> Option<String> {
        (url.starts_with("/p/default") || url.starts_with("/assets/")).then(|| {
            if url.starts_with("/assets/") {
                asset_name(url)
            } else {
                file_name(url, "default")
            }
        })
    }

    #[test]
    fn rewritten_pages_link_only_to_relative_files() {
        let origin = "http://127.0.0.1:4242";
        let html = concat!(
            "<html><head><link rel=\"stylesheet\" href=\"/assets/app.css\">",
            "<script src=\"/c.js\"></script><meta http-equiv=\"refresh\" content=\"30\"></head>",
            "<body><a href=\"/p/default/features/checkout?from=2026-01-01&amp;to=2026-02-01#top\">x</a>",
            "<a href=\"?tab=errors\">tab</a>",
            "<a href=\"http://127.0.0.1:4242/p/default/errors\">abs</a>",
            "<a href=\"/settings\">settings</a><a href=\"https://example.com/doc\">doc</a>",
            "<svg><use href=\"#i-gear\"></use></svg>",
            "<form method=\"post\" action=\"/p/default/timeline\"><input name=\"n\"><button>Add</button></form>",
            "<form action=\"/p/default/events\"><input name=\"q\"></form>",
            "<pre>curl http://127.0.0.1:4242/api</pre></body></html>"
        );
        let out = rewrite(html, "/p/default/churn", origin, &site_target);
        for bad in [
            "href=\"/",
            "src=\"/",
            "action=\"/",
            "method=\"post\"",
            "<script",
            "c.js",
            "refresh",
            "127.0.0.1",
        ] {
            assert!(!out.contains(bad), "{bad} left in {out}");
        }
        let deep = file_name(
            "/p/default/features/checkout?from=2026-01-01&to=2026-02-01",
            "default",
        );
        assert!(out.contains(&format!("href=\"{deep}#top\"")), "{out}");
        assert!(out.contains(&format!(
            "href=\"{}\"",
            file_name("/p/default/churn?tab=errors", "default")
        )));
        assert!(out.contains("href=\"https://example.com/doc\""));
        assert!(out.contains("href=\"#i-gear\""));
        assert!(out.contains("Read-only demo"));
        assert!(out.contains("<fieldset disabled"));
    }

    #[test]
    fn distinct_urls_get_distinct_flat_file_names() {
        let urls = [
            "/p/default",
            "/p/default/features",
            "/p/default/features/a",
            "/p/default/features.a",
            "/p/default/features?x=1",
            "/p/default/features?x=2",
            "/p/default/features?x=1&y=2",
            "/p/default/features?y=2&x=1",
            "/p/default/people/Ann%20B",
            "/p/default/people/ann-b",
            "/p/default/index",
        ];
        let names: BTreeSet<String> = urls.iter().map(|u| file_name(u, "default")).collect();
        assert_eq!(names.len(), urls.len(), "{names:?}");
        for n in &names {
            assert!(
                !n.contains('/') && !n.contains('?') && !n.contains('&'),
                "{n}"
            );
            assert!(n.len() < 120, "{n}");
        }
    }
}
