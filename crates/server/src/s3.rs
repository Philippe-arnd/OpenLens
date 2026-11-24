//! Optional second copy of the data on S3-compatible storage (D-016).
//!
//! Local disk stays the source of truth; this only uploads. Off unless
//! `OPENLENS_S3_BUCKET`, `OPENLENS_S3_ACCESS_KEY_ID` and
//! `OPENLENS_S3_SECRET_ACCESS_KEY` are set. Optional: `OPENLENS_S3_REGION`
//! (default `us-east-1`), `OPENLENS_S3_ENDPOINT` (for R2, MinIO, Garage…;
//! path-style addressing) and `OPENLENS_S3_PREFIX`.
//!
//! What goes up, from the background loop: every closed day of events
//! (`events/project=…/date=…/*.parquet`, once compacted; again if its files
//! change), the rollups of closed days (D-026) and a daily snapshot of
//! `meta.db`. Requests are signed with AWS
//! Signature Version 4, implemented here (no SDK).

use std::fs;
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Connection settings.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,
}

pub(crate) fn config() -> Option<Config> {
    let get = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    Some(Config {
        bucket: get("OPENLENS_S3_BUCKET")?,
        access_key: get("OPENLENS_S3_ACCESS_KEY_ID")?,
        secret_key: get("OPENLENS_S3_SECRET_ACCESS_KEY")?,
        region: get("OPENLENS_S3_REGION").unwrap_or_else(|| "us-east-1".into()),
        endpoint: get("OPENLENS_S3_ENDPOINT").map(|e| e.trim_end_matches('/').to_owned()),
        prefix: get("OPENLENS_S3_PREFIX")
            .map(|p| format!("{}/", p.trim_matches('/')))
            .unwrap_or_default(),
    })
}

/// Settings saved from the dashboard (#218), used when no `OPENLENS_S3_*`
/// variable is set. The secret is sealed ([`crate::secrets`]).
pub(crate) const KEYS: [&str; 6] = [
    "bucket",
    "region",
    "endpoint",
    "prefix",
    "access_key",
    "secret",
];

/// Where the S3 settings come from: the environment wins over the
/// dashboard (shown as read-only there).
pub(crate) fn configured(
    meta: &openlens_meta::Meta,
    data_dir: &Path,
) -> Option<(Config, &'static str)> {
    if let Some(c) = config() {
        return Some((c, "environment"));
    }
    let get = |k: &str| {
        meta.setting(&format!("s3:{k}"))
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    };
    let secret = crate::secrets::open(data_dir, &get("secret")?).ok()?;
    Some((
        Config {
            bucket: get("bucket")?,
            access_key: get("access_key")?,
            secret_key: secret,
            region: get("region").unwrap_or_else(|| "us-east-1".into()),
            endpoint: get("endpoint").map(|e| e.trim_end_matches('/').to_owned()),
            prefix: get("prefix")
                .map(|p| format!("{}/", p.trim_matches('/')))
                .unwrap_or_default(),
        },
        "dashboard",
    ))
}

/// Remember how the last copy went (shown on the Storage page).
pub(crate) fn record(meta: &openlens_meta::Meta, result: &Result<usize, String>) {
    let now = chrono::Utc::now().to_rfc3339();
    match result {
        Ok(n) => {
            let _ = meta.set_setting("s3:last_ok", &now);
            let _ = meta.set_setting("s3:last_objects", &n.to_string());
            let _ = meta.set_setting("s3:last_error", "");
        }
        Err(e) => {
            let _ = meta.set_setting("s3:last_error", &format!("{now} {e}"));
        }
    }
}

fn hmac(key: &[u8], msg: &str) -> Vec<u8> {
    match Hmac::<Sha256>::new_from_slice(key) {
        Ok(mut m) => {
            m.update(msg.as_bytes());
            m.finalize().into_bytes().to_vec()
        }
        Err(_) => Vec::new(),
    }
}

fn sha_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// URI-encode per SigV4 (unreserved characters kept; `/` kept in paths).
fn uri_encode(s: &str, keep_slash: bool) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b'/' if keep_slash => "/".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The `Authorization` header for a request (SigV4, service `s3`).
/// `headers` must include `host`, `x-amz-date` and `x-amz-content-sha256`;
/// names lowercase.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn authorization(
    access_key: &str,
    secret_key: &str,
    region: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
) -> String {
    authorization_q(
        access_key,
        secret_key,
        region,
        method,
        path,
        "",
        headers,
        payload_hash,
    )
}

/// [`authorization`] with a canonical query string (`a=1&b=2`, sorted,
/// already encoded).
#[allow(clippy::too_many_arguments)]
pub(crate) fn authorization_q(
    access_key: &str,
    secret_key: &str,
    region: &str,
    method: &str,
    path: &str,
    query: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
) -> String {
    let amz_date = headers
        .iter()
        .find(|h| h.0 == "x-amz-date")
        .map_or("", |h| h.1);
    let day = amz_date.get(..8).unwrap_or("");
    let mut hs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.trim().to_owned()))
        .collect();
    hs.sort();
    let canonical_headers: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed: Vec<&str> = hs.iter().map(|(k, _)| k.as_str()).collect();
    let signed = signed.join(";");
    let canonical = format!(
        "{method}\n{}\n{query}\n{canonical_headers}\n{signed}\n{payload_hash}",
        uri_encode(path, true)
    );
    let scope = format!("{day}/{region}/s3/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha_hex(canonical.as_bytes())
    );
    let k = hmac(format!("AWS4{secret_key}").as_bytes(), day);
    let k = hmac(&k, region);
    let k = hmac(&k, "s3");
    let k = hmac(&k, "aws4_request");
    let sig = hex::encode(hmac(&k, &to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed}, Signature={sig}"
    )
}

/// The URL, date and `Authorization` of a request whose body hashes to
/// `hash`. `key` is under the prefix; `query` pairs are sorted and encoded
/// here.
fn prepare(
    cfg: &Config,
    method: &str,
    key: &str,
    query: &[(&str, &str)],
    hash: &str,
) -> (String, String, String) {
    // A list call targets the bucket root: its prefix travels in the query.
    let key = if key.is_empty() {
        String::new()
    } else {
        format!("{}{key}", cfg.prefix)
    };
    let (base, host, path) = match &cfg.endpoint {
        Some(ep) => {
            let host = ep.split("://").nth(1).unwrap_or(ep).to_owned();
            let path = if key.is_empty() {
                format!("/{}", cfg.bucket)
            } else {
                format!("/{}/{key}", cfg.bucket)
            };
            (ep.clone(), host, path)
        }
        None => {
            let host = format!("{}.s3.{}.amazonaws.com", cfg.bucket, cfg.region);
            (format!("https://{host}"), host, format!("/{key}"))
        }
    };
    let mut q: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    q.sort();
    let qs = q
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let auth = authorization_q(
        &cfg.access_key,
        &cfg.secret_key,
        &cfg.region,
        method,
        &path,
        &qs,
        &[
            ("host", &host),
            ("x-amz-content-sha256", hash),
            ("x-amz-date", &now),
        ],
        hash,
    );
    let url = format!(
        "{base}{}{}{qs}",
        uri_encode(&path, true),
        if qs.is_empty() { "" } else { "?" }
    );
    (url, now, auth)
}

/// One signed request: `(status, body)`. `key` is under the prefix;
/// `query` pairs are sorted and encoded here.
pub(crate) fn request(
    cfg: &Config,
    method: &str,
    key: &str,
    query: &[(&str, &str)],
    body: &[u8],
) -> Result<(u16, Vec<u8>), String> {
    let hash = sha_hex(body);
    let (url, now, auth) = prepare(cfg, method, key, query, &hash);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let r = match method {
        "PUT" => agent
            .put(&url)
            .header("x-amz-date", &now)
            .header("x-amz-content-sha256", &hash)
            .header("authorization", &auth)
            .send(body),
        "GET" => agent
            .get(&url)
            .header("x-amz-date", &now)
            .header("x-amz-content-sha256", &hash)
            .header("authorization", &auth)
            .call(),
        "DELETE" => agent
            .delete(&url)
            .header("x-amz-date", &now)
            .header("x-amz-content-sha256", &hash)
            .header("authorization", &auth)
            .call(),
        _ => return Err("unsupported method".into()),
    };
    let mut r = r.map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    let bytes = r
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    Ok((status, bytes))
}

fn ok(r: (u16, Vec<u8>), what: &str) -> Result<Vec<u8>, String> {
    if (200..300).contains(&r.0) {
        Ok(r.1)
    } else {
        let text = String::from_utf8_lossy(&r.1);
        let code = text
            .split("<Code>")
            .nth(1)
            .and_then(|t| t.split("</Code>").next())
            .unwrap_or("");
        Err(format!("{what}: HTTP {} {code}", r.0))
    }
}

/// Upload one object.
pub(crate) fn put(cfg: &Config, key: &str, body: &[u8]) -> Result<(), String> {
    ok(request(cfg, "PUT", key, &[], body)?, "upload").map(|_| ())
}

/// Upload a file without holding it in memory: hashed in a first pass,
/// then streamed (a backup archive can be larger than RAM; review of
/// v0.3.0).
pub(crate) fn put_file(cfg: &Config, key: &str, path: &std::path::Path) -> Result<(), String> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let hash = hex::encode(h.finalize());
    let (url, now, auth) = prepare(cfg, "PUT", key, &[], &hash);
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(6 * 3600)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut r = agent
        .put(&url)
        .header("x-amz-date", &now)
        .header("x-amz-content-sha256", &hash)
        .header("authorization", &auth)
        .send(file)
        .map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    let body = r
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    ok((status, body), "upload").map(|_| ())
}

/// Read one object.
pub(crate) fn get(cfg: &Config, key: &str) -> Result<Vec<u8>, String> {
    ok(request(cfg, "GET", key, &[], &[])?, "read")
}

/// Delete one object.
pub(crate) fn delete(cfg: &Config, key: &str) -> Result<(), String> {
    ok(request(cfg, "DELETE", key, &[], &[])?, "delete").map(|_| ())
}

/// Keys under `prefix` (below the configured prefix), at most 1000.
pub(crate) fn list(cfg: &Config, prefix: &str) -> Result<Vec<String>, String> {
    let full = format!("{}{prefix}", cfg.prefix);
    let body = ok(
        request(
            cfg,
            "GET",
            "",
            &[("list-type", "2"), ("prefix", &full)],
            &[],
        )?,
        "list",
    )?;
    let text = String::from_utf8_lossy(&body);
    Ok(text
        .split("<Key>")
        .skip(1)
        .filter_map(|t| t.split("</Key>").next())
        .map(|k| {
            k.strip_prefix(&cfg.prefix)
                .unwrap_or(k)
                .replace("&amp;", "&")
        })
        .collect())
}

/// `(key, size)` of the objects under `prefix` (below the configured
/// prefix), at most 1000 (#326).
pub(crate) fn list_sized(cfg: &Config, prefix: &str) -> Result<Vec<(String, u64)>, String> {
    let full = format!("{}{prefix}", cfg.prefix);
    let body = ok(
        request(
            cfg,
            "GET",
            "",
            &[("list-type", "2"), ("prefix", &full)],
            &[],
        )?,
        "list",
    )?;
    let text = String::from_utf8_lossy(&body);
    let tag = |t: &str, name: &str| -> Option<String> {
        Some(
            t.split(&format!("<{name}>"))
                .nth(1)?
                .split(&format!("</{name}>"))
                .next()?
                .to_owned(),
        )
    };
    Ok(text
        .split("<Contents>")
        .skip(1)
        .filter_map(|c| {
            let key = tag(c, "Key")?.replace("&amp;", "&");
            let size = tag(c, "Size")?.parse().ok()?;
            Some((
                key.strip_prefix(&cfg.prefix).unwrap_or(&key).to_owned(),
                size,
            ))
        })
        .collect())
}

/// Put, read back and delete a probe object: are the settings right?
pub(crate) fn test(cfg: &Config) -> Result<(), String> {
    let key = format!(".openlens-probe-{}", chrono::Utc::now().timestamp());
    put(cfg, &key, b"openlens")?;
    let back = get(cfg, &key)?;
    delete(cfg, &key)?;
    if back != b"openlens" {
        return Err("read back something else".into());
    }
    Ok(())
}

const MARK: &str = ".s3-uploaded";

/// Upload the closed days of one tree (`<tree>/project=…/date=…/*.parquet`)
/// whose files changed since the last upload. With `stamp`, a file
/// rewritten under the same name counts as changed.
fn sync_tree(
    cfg: &Config,
    root: &Path,
    tree: &Path,
    today: &str,
    stamp: bool,
) -> Result<usize, String> {
    let mut sent = 0;
    let Ok(projects) = fs::read_dir(tree) else {
        return Ok(0);
    };
    for p in projects.flatten() {
        let Ok(days) = fs::read_dir(p.path()) else {
            continue;
        };
        for d in days.flatten() {
            let dir = d.path();
            let name = d.file_name().to_string_lossy().into_owned();
            if !name.starts_with("date=") || name == format!("date={today}") {
                continue;
            }
            let mut files: Vec<PathBuf> = fs::read_dir(&dir)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.path())
                .filter(|f| f.extension().is_some_and(|x| x == "parquet"))
                .collect();
            files.sort();
            let listing: String = files
                .iter()
                .filter_map(|f| {
                    let n = f.file_name()?.to_string_lossy().into_owned();
                    if !stamp {
                        return Some(n + "\n");
                    }
                    let m = fs::metadata(f).ok()?;
                    let t = m
                        .modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_nanos();
                    Some(format!("{n} {} {t}\n", m.len()))
                })
                .collect();
            if fs::read_to_string(dir.join(MARK)).ok().as_deref() == Some(listing.as_str()) {
                continue;
            }
            for f in &files {
                let rel = f
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                put(cfg, &rel, &fs::read(f).map_err(|e| e.to_string())?)?;
                sent += 1;
            }
            fs::write(dir.join(MARK), listing).map_err(|e| e.to_string())?;
        }
    }
    Ok(sent)
}

/// Upload closed days whose files changed since the last upload, and the
/// day's metadata snapshot. Returns the number of objects uploaded.
pub(crate) fn sync(
    cfg: &Config,
    root: &Path,
    meta: &openlens_meta::Meta,
    today: &str,
) -> Result<usize, String> {
    let mut sent = sync_tree(cfg, root, &root.join("events"), today, false)?;
    // The rollups outlive the events (D-026): copy them too. They are
    // rewritten in place when someone is erased, so their mark records
    // sizes and times, not only names.
    let rollups = root
        .join("rollups")
        .join(format!("v{}", openlens_storage::rollup::ROLLUP_VERSION));
    sent += sync_tree(cfg, root, &rollups, today, true)?;
    let snap_mark = root.join(".s3-meta-day");
    if fs::read_to_string(&snap_mark).ok().as_deref() != Some(today) {
        let tmp = root.join(format!(".meta-snapshot-{today}.db"));
        let _ = fs::remove_file(&tmp);
        meta.backup_to(&tmp).map_err(|e| e.to_string())?;
        let body = fs::read(&tmp).map_err(|e| e.to_string());
        let _ = fs::remove_file(&tmp);
        put(cfg, &format!("meta/meta-{today}.db"), &body?)?;
        fs::write(&snap_mark, today).map_err(|e| e.to_string())?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    /// A fake S3 endpoint recording `PUT` paths and whether they were signed.
    /// `(path, signed)` of each request the fake endpoint received.
    type Seen = Arc<Mutex<Vec<(String, bool)>>>;

    fn fake_s3() -> (String, Seen) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s);
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (mut len, mut signed) = (0usize, false);
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    signed |= lower.starts_with("authorization: aws4-hmac-sha256 credential=ak/");
                    if h.trim().is_empty() {
                        break;
                    }
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                g.lock().unwrap().push((path, signed));
                r.get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .unwrap();
            }
        });
        (url, got)
    }

    #[test]
    fn sync_uploads_closed_days_once_and_again_after_they_change() {
        let d = tempfile::tempdir().unwrap();
        let day = d.path().join("events/project=p/date=2026-11-20");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("a.parquet"), b"x").unwrap();
        let today = d.path().join("events/project=p/date=2026-11-22");
        fs::create_dir_all(&today).unwrap();
        fs::write(today.join("t.parquet"), b"x").unwrap();
        let meta = openlens_meta::Meta::in_memory().unwrap();
        let (url, got) = fake_s3();
        let cfg = Config {
            bucket: "lens".into(),
            region: "auto".into(),
            endpoint: Some(url),
            prefix: "prod/".into(),
            access_key: "ak".into(),
            secret_key: "sk".into(),
        };
        assert_eq!(sync(&cfg, d.path(), &meta, "2026-11-22").unwrap(), 2);
        let paths: Vec<String> = got.lock().unwrap().iter().map(|p| p.0.clone()).collect();
        assert!(
            paths.contains(&"/lens/prod/events/project%3Dp/date%3D2026-11-20/a.parquet".to_owned()),
            "{paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p == "/lens/prod/meta/meta-2026-11-22.db")
        );
        assert!(
            !paths.iter().any(|p| p.contains("2026-11-22/t.parquet")),
            "today is not closed"
        );
        assert!(
            got.lock().unwrap().iter().all(|p| p.1),
            "every request is signed"
        );
        assert_eq!(
            sync(&cfg, d.path(), &meta, "2026-11-22").unwrap(),
            0,
            "nothing changed"
        );
        fs::write(day.join("b.parquet"), b"y").unwrap();
        assert_eq!(
            sync(&cfg, d.path(), &meta, "2026-11-22").unwrap(),
            2,
            "a compacted day goes up again"
        );
        // Rollups go up too, and again when rewritten under the same name
        // (someone erased, D-026).
        let r = d.path().join("rollups/v1/project=p/date=2025-01-01");
        fs::create_dir_all(&r).unwrap();
        fs::write(r.join("person_q.parquet"), b"r1").unwrap();
        assert_eq!(sync(&cfg, d.path(), &meta, "2026-11-22").unwrap(), 1);
        assert!(
            got.lock()
                .unwrap()
                .iter()
                .any(|p| p.0
                    == "/lens/prod/rollups/v1/project%3Dp/date%3D2025-01-01/person_q.parquet")
        );
        assert_eq!(sync(&cfg, d.path(), &meta, "2026-11-22").unwrap(), 0);
        fs::write(r.join("person_q.parquet"), b"r2-trimmed").unwrap();
        assert_eq!(sync(&cfg, d.path(), &meta, "2026-11-22").unwrap(), 1);
    }

    /// The worked "PUT Object" example of the AWS SigV4 documentation.
    #[test]
    fn matches_the_aws_documentation_example() {
        let body_hash = sha_hex(b"Welcome to Amazon S3.");
        assert_eq!(
            body_hash,
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
        );
        let auth = authorization(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "PUT",
            "/test$file.text",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("date", "Fri, 24 May 2013 00:00:00 GMT"),
                ("x-amz-date", "20130524T000000Z"),
                ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
                ("x-amz-content-sha256", &body_hash),
            ],
            &body_hash,
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class, \
             Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }
}
