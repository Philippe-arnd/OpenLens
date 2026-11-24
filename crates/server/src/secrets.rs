//! Secrets stored in the metadata store (#218: the S3 secret key), sealed
//! with ChaCha20-Poly1305. The key is `OPENLENS_SECRET_KEY` (64 hex
//! characters) when set, else `<data>/.secret.key`, made on first use
//! (mode 0600). Backups copy neither, so a restored store asks for its S3
//! secret again instead of carrying it to wherever the backup goes.

use std::path::Path;

use ring::aead::{CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

const FILE: &str = ".secret.key";
const SESSION_FILE: &str = ".session.key";

fn key(data_dir: &Path) -> Result<LessSafeKey, String> {
    let bytes = match std::env::var("OPENLENS_SECRET_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(hexkey) => {
            hex::decode(hexkey.trim()).map_err(|_| "OPENLENS_SECRET_KEY must be hex")?
        }
        None => {
            let path = data_dir.join(FILE);
            match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => {
                    let mut b = vec![0u8; 32];
                    SystemRandom::new()
                        .fill(&mut b)
                        .map_err(|_| "no randomness")?;
                    write_private(&path, &b)?;
                    b
                }
            }
        }
    };
    if bytes.len() != 32 {
        return Err("the secret key must be 32 bytes (64 hex characters)".into());
    }
    let k = UnboundKey::new(&CHACHA20_POLY1305, &bytes).map_err(|_| "bad key")?;
    Ok(LessSafeKey::new(k))
}

fn write_private(path: &Path, b: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, b).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// The key that signs session cookies and the OIDC flow:
/// `<data>/.session.key`, made on first use (mode 0600). It is not the
/// admin token, so a copy of the store or a log line with the token cannot
/// forge a session, and backups (which copy only `meta.db` and the data
/// trees) do not carry it. A new key signs everyone out.
pub fn session_key(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join(SESSION_FILE);
    if let Ok(b) = std::fs::read(&path)
        && b.len() == 32
    {
        return Ok(hex::encode(b));
    }
    let mut b = vec![0u8; 32];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| "no randomness")?;
    write_private(&path, &b)?;
    Ok(hex::encode(b))
}

/// Seal `plain`: `hex(nonce || ciphertext || tag)`.
pub(crate) fn seal(data_dir: &Path, plain: &str) -> Result<String, String> {
    let k = key(data_dir)?;
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "no randomness")?;
    let mut buf = plain.as_bytes().to_vec();
    k.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        ring::aead::Aad::empty(),
        &mut buf,
    )
    .map_err(|_| "seal failed")?;
    let mut out = nonce.to_vec();
    out.extend(buf);
    Ok(hex::encode(out))
}

/// Open a value made by [`seal`]. Fails when the key changed.
pub(crate) fn open(data_dir: &Path, sealed: &str) -> Result<String, String> {
    let k = key(data_dir)?;
    let raw = hex::decode(sealed).map_err(|_| "not a sealed value")?;
    if raw.len() < NONCE_LEN + 16 {
        return Err("not a sealed value".into());
    }
    let (n, rest) = raw.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(n).map_err(|_| "bad nonce")?;
    let mut buf = rest.to_vec();
    let plain = k
        .open_in_place(nonce, ring::aead::Aad::empty(), &mut buf)
        .map_err(|_| "the secret key changed: enter the secret again")?;
    String::from_utf8(plain.to_vec()).map_err(|_| "not text".into())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    #[test]
    fn seals_and_opens_and_a_new_key_cannot_open() {
        let d = tempfile::tempdir().unwrap();
        let s = super::seal(d.path(), "s3cr3t").unwrap();
        assert!(!s.contains("s3cr3t"));
        assert_eq!(super::open(d.path(), &s).unwrap(), "s3cr3t");
        assert_ne!(super::seal(d.path(), "s3cr3t").unwrap(), s, "fresh nonce");
        let other = tempfile::tempdir().unwrap();
        assert!(super::open(other.path(), &s).is_err());
        assert!(super::open(d.path(), "zz").is_err());
    }
}
