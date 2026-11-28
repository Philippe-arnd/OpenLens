//! Audience enrichment at capture (#225): country, device class, browser
//! and OS added to `$pageview` events. The client address is used for the
//! lookup and never stored. The country database is the free DB-IP Lite
//! country file (CC BY 4.0, credited in ATTRIBUTION.md and on the page),
//! shipped in the image; without it, the country is left out.

use std::net::IpAddr;
use std::path::Path;
use std::sync::OnceLock;

use axum::http::HeaderMap;
use maxminddb::{PathElement, Reader};

static DB: OnceLock<Option<Reader<Vec<u8>>>> = OnceLock::new();

/// Where the country database is looked for, first found wins.
fn candidates(data_dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("OPENLENS_GEOIP_DB")
        && !p.trim().is_empty()
    {
        v.push(p.into());
    }
    v.push("/usr/share/openlens/dbip-country-lite.mmdb.gz".into());
    v.push(data_dir.join("dbip-country-lite.mmdb.gz"));
    v.push(data_dir.join("dbip-country-lite.mmdb"));
    v
}

/// Load the database once (gzip or plain).
pub(crate) fn init(data_dir: &Path) {
    DB.get_or_init(|| {
        for p in candidates(data_dir) {
            let Ok(raw) = std::fs::read(&p) else { continue };
            let bytes = if raw.starts_with(&[0x1f, 0x8b]) {
                let mut out = Vec::new();
                if std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&raw[..]), &mut out)
                    .is_err()
                {
                    tracing::warn!(path = %p.display(), "country database unreadable");
                    continue;
                }
                out
            } else {
                raw
            };
            match Reader::from_source(bytes) {
                Ok(r) => {
                    tracing::info!(path = %p.display(), "country database loaded");
                    return Some(r);
                }
                Err(e) => tracing::warn!(path = %p.display(), %e, "country database invalid"),
            }
        }
        None
    });
}

/// Is a country database loaded?
pub(crate) fn has_geo() -> bool {
    DB.get().is_some_and(Option::is_some)
}

/// ISO country code of an address, when known.
pub(crate) fn country(ip: IpAddr) -> Option<String> {
    let r = DB.get()?.as_ref()?;
    let res = r.lookup(ip).ok()?;
    let code: String = res
        .decode_path(&[PathElement::Key("country"), PathElement::Key("iso_code")])
        .ok()??;
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}

/// The client address: the last `X-Forwarded-For` entry (the one our
/// proxy added), else `X-Real-IP`. Direct connections have none.
pub(crate) fn client_ip(peer: &crate::capture::Peer, headers: &HeaderMap) -> Option<IpAddr> {
    // The same rule as the rate limits (D-022): forwarded headers count only
    // from a proxy, and the peer is used without one (review of v0.3.0).
    crate::capture::client_ip(peer, headers).and_then(|s| s.parse().ok())
}

/// Device class, browser and OS from a user agent (a few rules, in order;
/// unknown stays out).
pub(crate) fn parse_ua(
    ua: &str,
) -> (
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
) {
    let has = |s: &str| ua.contains(s);
    let os = if has("Windows") {
        Some("Windows")
    } else if has("iPhone") || has("iPad") || has("iPod") {
        Some("iOS")
    } else if has("Android") {
        Some("Android")
    } else if has("CrOS") {
        Some("ChromeOS")
    } else if has("Mac OS X") || has("Macintosh") {
        Some("macOS")
    } else if has("Linux") {
        Some("Linux")
    } else {
        None
    };
    let browser = if has("Edg/") || has("EdgA/") || has("EdgiOS/") {
        Some("Edge")
    } else if has("OPR/") || has("Opera") {
        Some("Opera")
    } else if has("SamsungBrowser/") {
        Some("Samsung Internet")
    } else if has("Firefox/") || has("FxiOS/") {
        Some("Firefox")
    } else if has("Chrome/") || has("CriOS/") {
        Some("Chrome")
    } else if has("Safari/") {
        Some("Safari")
    } else {
        None
    };
    let device = if has("iPad") || (has("Android") && !has("Mobile")) || has("Tablet") {
        Some("tablet")
    } else if has("Mobi") || has("iPhone") || has("iPod") || has("Android") {
        Some("phone")
    } else if os.is_some() {
        Some("desktop")
    } else {
        None
    };
    (device, browser, os)
}

/// The host of a URL, lowercased, without `www.` (None when not http(s)).
pub(crate) fn host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let h = rest.split(['/', '?', '#']).next()?.split('@').next_back()?;
    let h = h.split(':').next()?.to_ascii_lowercase();
    let h = h.strip_prefix("www.").unwrap_or(&h).to_owned();
    (!h.is_empty() && h.len() <= 253).then_some(h)
}

/// The channel of a landing (#226), from its campaign parameters and the
/// referrer's host; no rules to write.
pub(crate) fn channel(
    utm_source: Option<&str>,
    utm_medium: Option<&str>,
    click_id: bool,
    ref_host: Option<&str>,
    own_host: Option<&str>,
) -> &'static str {
    let medium = utm_medium.unwrap_or("").to_ascii_lowercase();
    let source = utm_source.unwrap_or("").to_ascii_lowercase();
    let host = ref_host.unwrap_or("");
    let ends = |list: &[&str]| {
        list.iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    };
    let search = [
        "google.com",
        "bing.com",
        "duckduckgo.com",
        "yahoo.com",
        "ecosia.org",
        "qwant.com",
        "baidu.com",
        "yandex.ru",
        "yandex.com",
        "startpage.com",
        "search.brave.com",
        "kagi.com",
    ];
    let social = [
        "facebook.com",
        "instagram.com",
        "t.co",
        "twitter.com",
        "x.com",
        "linkedin.com",
        "lnkd.in",
        "reddit.com",
        "youtube.com",
        "tiktok.com",
        "pinterest.com",
        "mastodon.social",
        "threads.net",
        "bsky.app",
        "news.ycombinator.com",
        "discord.com",
        "whatsapp.com",
        "telegram.org",
    ];
    let ai = [
        "chatgpt.com",
        "chat.openai.com",
        "perplexity.ai",
        "claude.ai",
        "gemini.google.com",
        "copilot.microsoft.com",
        "you.com",
        "phind.com",
        "mistral.ai",
        "chat.mistral.ai",
    ];
    let mail = [
        "mail.google.com",
        "outlook.live.com",
        "outlook.office.com",
        "mail.yahoo.com",
        "mail.proton.me",
    ];
    if click_id
        || [
            "cpc",
            "ppc",
            "paid",
            "display",
            "cpm",
            "paidsearch",
            "paid_social",
            "paidsocial",
        ]
        .iter()
        .any(|m| medium.contains(m))
    {
        return "paid";
    }
    if medium.contains("email")
        || medium.contains("newsletter")
        || source.contains("newsletter")
        || ends(&mail)
    {
        return "email";
    }
    if !host.is_empty() && own_host.is_some_and(|o| o == host) {
        return "internal";
    }
    if ends(&ai)
        || ["chatgpt", "perplexity", "claude", "gemini", "copilot"]
            .iter()
            .any(|a| source.contains(a))
    {
        return "ai";
    }
    if ends(&search)
        || (host.contains(".google.") || host.starts_with("google."))
        || medium == "organic"
    {
        return "search";
    }
    if ends(&social) || medium.contains("social") {
        return "social";
    }
    if !host.is_empty() || !source.is_empty() {
        return "referral";
    }
    "direct"
}

/// Add `$country`, `$device`, `$browser` and `$os` to the page views of a
/// batch (server values replace whatever the client sent under those
/// names). Other events are left as they are. `rules` are the project's
/// host rules: a referrer on one of its hosts is `internal`.
pub(crate) fn enrich(
    events: &mut [openlens_event::Event],
    peer: &crate::capture::Peer,
    headers: &HeaderMap,
    rules: &openlens_engine::environments::Rules,
) {
    if !events.iter().any(|e| e.event == "$pageview") {
        return;
    }
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let (device, browser, os) = parse_ua(ua);
    let country = client_ip(peer, headers).and_then(country);
    for e in events.iter_mut().filter(|e| e.event == "$pageview") {
        let Ok(serde_json::Value::Object(mut props)) =
            serde_json::from_str::<serde_json::Value>(&e.properties)
        else {
            continue;
        };
        for (k, v) in [
            ("$country", country.as_deref()),
            ("$device", device),
            ("$browser", browser),
            ("$os", os),
        ] {
            props.remove(k);
            if let Some(v) = v {
                props.insert(k.into(), serde_json::Value::String(v.into()));
            }
        }
        // The first view of a page load (it carries `$lang`) gets its
        // channel; only the referrer's host is kept (#226).
        if props.contains_key("$lang") {
            let get = |k: &str| props.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            let ref_host = get("$referrer").as_deref().and_then(host);
            // A referrer on another of the project's hosts (the landing
            // page, #710, D-112) is internal like the page's own host.
            let sister = get("$referrer")
                .as_deref()
                .and_then(openlens_event::host_of)
                .is_some_and(|h| rules.is_own_host(&h));
            let own = if sister {
                ref_host.clone()
            } else {
                e.url.as_deref().and_then(host)
            };
            let click = ["$gclid", "$fbclid", "$msclkid"]
                .iter()
                .any(|k| props.contains_key(*k));
            let ch = channel(
                get("$utm_source").as_deref(),
                get("$utm_medium").as_deref(),
                click,
                ref_host.as_deref(),
                own.as_deref(),
            );
            props.remove("$referrer");
            props.remove("$ref_host");
            props.remove("$channel");
            if let Some(h) = ref_host {
                props.insert("$ref_host".into(), serde_json::Value::String(h));
            }
            props.insert("$channel".into(), serde_json::Value::String(ch.into()));
        }
        if let Ok(s) = serde_json::to_string(&serde_json::Value::Object(props)) {
            e.properties = s;
        }
    }
}

#[cfg(test)]
mod channel_tests {
    use super::{channel, host};

    #[test]
    fn groups_landings_into_channels() {
        assert_eq!(
            host("https://www.Google.com/search?q=x").as_deref(),
            Some("google.com")
        );
        assert_eq!(host("ftp://x"), None);
        let c = |s: Option<&str>, m: Option<&str>, click: bool, h: Option<&str>| {
            channel(s, m, click, h, Some("app.example.com"))
        };
        assert_eq!(c(None, None, false, None), "direct");
        assert_eq!(c(None, None, false, Some("google.com")), "search");
        assert_eq!(c(None, None, false, Some("google.fr")), "search");
        assert_eq!(c(None, None, false, Some("news.ycombinator.com")), "social");
        assert_eq!(c(None, None, false, Some("chatgpt.com")), "ai");
        assert_eq!(c(Some("newsletter"), Some("email"), false, None), "email");
        assert_eq!(
            c(Some("google"), Some("cpc"), false, Some("google.com")),
            "paid"
        );
        assert_eq!(c(None, None, true, Some("facebook.com")), "paid");
        assert_eq!(c(None, None, false, Some("app.example.com")), "internal");
        assert_eq!(c(None, None, false, Some("blog.someone.dev")), "referral");
        assert_eq!(c(Some("partner"), None, false, None), "referral");
    }
}

#[cfg(test)]
mod tests {
    use super::parse_ua;

    #[test]
    fn reads_common_user_agents() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";
        assert_eq!(
            parse_ua(iphone),
            (Some("phone"), Some("Safari"), Some("iOS"))
        );
        let win_edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0";
        assert_eq!(
            parse_ua(win_edge),
            (Some("desktop"), Some("Edge"), Some("Windows"))
        );
        let mac_ff =
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.5; rv:132.0) Gecko/20100101 Firefox/132.0";
        assert_eq!(
            parse_ua(mac_ff),
            (Some("desktop"), Some("Firefox"), Some("macOS"))
        );
        let tab = "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
        assert_eq!(
            parse_ua(tab),
            (Some("tablet"), Some("Chrome"), Some("Android"))
        );
        let phone = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36";
        assert_eq!(
            parse_ua(phone),
            (Some("phone"), Some("Chrome"), Some("Android"))
        );
        assert_eq!(parse_ua("curl/8"), (None, None, None));
    }
}
