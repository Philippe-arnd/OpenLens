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
//! change) and a daily snapshot of `meta.db`. Requests are signed with AWS
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
pub(crate) fn authorization(
    access_key: &str,
    secret_key: &str,
    region: &str,
    method: &str,
    path: &str,
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
        "{method}\n{}\n\n{canonical_headers}\n{signed}\n{payload_hash}",
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

/// Upload one object.
pub(crate) fn put(cfg: &Config, key: &str, body: &[u8]) -> Result<(), String> {
    let key = format!("{}{key}", cfg.prefix);
    let (base, host, path) = match &cfg.endpoint {
        Some(ep) => {
            let host = ep.split("://").nth(1).unwrap_or(ep).to_owned();
            (ep.clone(), host, format!("/{}/{key}", cfg.bucket))
        }
        None => {
            let host = format!("{}.s3.{}.amazonaws.com", cfg.bucket, cfg.region);
            (format!("https://{host}"), host, format!("/{key}"))
        }
    };
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let hash = sha_hex(body);
    let auth = authorization(
        &cfg.access_key,
        &cfg.secret_key,
        &cfg.region,
        "PUT",
        &path,
        &[
            ("host", &host),
            ("x-amz-content-sha256", &hash),
            ("x-amz-date", &now),
        ],
        &hash,
    );
    ureq::put(&format!("{base}{}", uri_encode(&path, true)))
        .header("x-amz-date", &now)
        .header("x-amz-content-sha256", &hash)
        .header("authorization", &auth)
        .send(body)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

const MARK: &str = ".s3-uploaded";

/// Upload closed days whose files changed since the last upload, and the
/// day's metadata snapshot. Returns the number of objects uploaded.
pub(crate) fn sync(
    cfg: &Config,
    root: &Path,
    meta: &openlens_meta::Meta,
    today: &str,
) -> Result<usize, String> {
    let mut sent = 0;
    let events = root.join("events");
    let Ok(projects) = fs::read_dir(&events) else {
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
                    f.file_name()
                        .map(|n| n.to_string_lossy().into_owned() + "\n")
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
