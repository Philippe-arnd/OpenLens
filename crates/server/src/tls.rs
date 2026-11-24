//! When does the app's TLS certificate expire? (#266) A TLS handshake with
//! the app address (public addresses only, like every outbound request),
//! then the certificate's `notAfter`, read with a few lines of DER.

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// Read one DER element at `pos`: `(tag, content start, content end)`.
fn der(b: &[u8], pos: usize) -> Option<(u8, usize, usize)> {
    let tag = *b.get(pos)?;
    let first = *b.get(pos + 1)?;
    let (len, start) = if first < 0x80 {
        (usize::from(first), pos + 2)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | usize::from(*b.get(pos + 2 + i)?);
        }
        (len, pos + 2 + n)
    };
    let end = start.checked_add(len)?;
    (end <= b.len()).then_some((tag, start, end))
}

/// `notAfter` of an X.509 certificate (DER), as Unix seconds.
pub(crate) fn not_after(cert: &[u8]) -> Option<i64> {
    let (_, c0, _) = der(cert, 0)?; // Certificate
    let (_, mut p, tbs_end) = der(cert, c0)?; // TBSCertificate
    // Optional [0] version.
    if cert.get(p) == Some(&0xa0) {
        p = der(cert, p)?.2;
    }
    p = der(cert, p)?.2; // serialNumber
    p = der(cert, p)?.2; // signature algorithm
    p = der(cert, p)?.2; // issuer
    let (_, v0, _) = der(cert, p)?; // validity
    let (_, _, nb_end) = der(cert, v0)?; // notBefore
    let (tag, s, e) = der(cert, nb_end)?; // notAfter
    if e > tbs_end {
        return None;
    }
    let text = std::str::from_utf8(&cert[s..e]).ok()?;
    let fmt = match tag {
        0x17 => {
            // UTCTime YYMMDDHHMMSSZ: years 50–99 are 19xx.
            let yy: i32 = text.get(..2)?.parse().ok()?;
            let full = format!("{}{}", if yy >= 50 { "19" } else { "20" }, text);
            return chrono::NaiveDateTime::parse_from_str(
                full.trim_end_matches('Z'),
                "%Y%m%d%H%M%S",
            )
            .ok()
            .map(|d| d.and_utc().timestamp());
        }
        0x18 => "%Y%m%d%H%M%S",
        _ => return None,
    };
    chrono::NaiveDateTime::parse_from_str(text.trim_end_matches('Z'), fmt)
        .ok()
        .map(|d| d.and_utc().timestamp())
}

/// The expiry of the certificate served for an https origin. Only public
/// addresses are reached; any failure is an error string.
pub(crate) fn expiry(origin: &str) -> Result<i64, String> {
    let rest = origin
        .strip_prefix("https://")
        .ok_or("not an https address")?;
    // `[v6]:port`, `[v6]`, `host:port` or `host` (review of v0.3.0: a bare
    // `[2001:db8::1]` was cut at its last colon).
    let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or("bad address")?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse::<u16>().map_err(|_| "bad port")?,
            None if after.is_empty() => 443,
            None => return Err("bad address".into()),
        };
        (h.to_owned(), port)
    } else {
        match rest.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), p.parse::<u16>().map_err(|_| "bad port")?),
            None => (rest.to_owned(), 443),
        }
    };
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .find(|a| crate::icon::public(a.ip()))
        .ok_or("no public address")?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let mut conn =
        rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    let mut sock =
        TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|e| e.to_string())?;
    sock.set_write_timeout(Some(Duration::from_secs(8)))
        .map_err(|e| e.to_string())?;
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);
    // Writing nothing still drives the handshake to completion.
    tls.flush().map_err(|e| e.to_string())?;
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(tls.sock).map_err(|e| e.to_string())?;
    }
    let certs = tls.conn.peer_certificates().ok_or("no certificate")?;
    let leaf = certs.first().ok_or("no certificate")?;
    not_after(leaf.as_ref()).ok_or_else(|| "unreadable certificate".to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_not_after_from_der() {
        // A minimal certificate skeleton: version, serial, alg, issuer,
        // validity (UTCTime notBefore, GeneralizedTime notAfter).
        let not_before = b"240101000000Z";
        let not_after = b"20271231235959Z";
        let mut validity = vec![0x17, not_before.len() as u8];
        validity.extend_from_slice(not_before);
        validity.push(0x18);
        validity.push(not_after.len() as u8);
        validity.extend_from_slice(not_after);
        let mut tbs = vec![
            0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01, 0x07, 0x30, 0x00, 0x30, 0x00,
        ];
        tbs.push(0x30);
        tbs.push(validity.len() as u8);
        tbs.extend(validity);
        let mut tbs_seq = vec![0x30, tbs.len() as u8];
        tbs_seq.extend(tbs);
        let mut cert = vec![0x30, tbs_seq.len() as u8];
        cert.extend(tbs_seq);
        let t = super::not_after(&cert);
        assert_eq!(
            t,
            chrono::NaiveDate::from_ymd_opt(2027, 12, 31)
                .and_then(|d| d.and_hms_opt(23, 59, 59))
                .map(|d| d.and_utc().timestamp())
        );
        assert_eq!(super::not_after(&[0x30, 0x05, 0x01]), None);
    }

    /// Needs the network: `cargo test -p openlens-server --lib tls -- --ignored`.
    #[test]
    #[ignore = "network"]
    fn reads_a_live_certificate() {
        let t = super::expiry("https://crates.io");
        assert!(
            t.as_ref()
                .is_ok_and(|t| *t > chrono::Utc::now().timestamp()),
            "{t:?}"
        );
        assert!(super::expiry("http://crates.io").is_err());
    }
}
