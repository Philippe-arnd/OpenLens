//! Test helpers: synthetic event generators and hostile-input corpora.
//!
//! Only dev-dependencies may use this crate (`cargo xtask layers`).

/// Hostile JSON bodies for `/capture`. Every crash fix adds its input here.
pub fn hostile_capture_bodies() -> Vec<Vec<u8>> {
    let deep = format!(
        "{{\"batch\":[{{\"event\":\"x\",\"distinct_id\":\"u\",\"properties\":{{\"a\":{}1{}}}}}]}}",
        "[".repeat(5_000),
        "]".repeat(5_000)
    );
    let wide = format!(
        "{{\"batch\":[{}]}}",
        vec!["{\"event\":\"x\",\"distinct_id\":\"u\"}"; 1_001].join(",")
    );
    vec![
        Vec::new(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{\"batch\":null}".to_vec(),
        b"{\"batch\":[null]}".to_vec(),
        b"{\"batch\":[{\"event\":123}]}".to_vec(),
        b"{\"batch\":[{\"event\":\"\",\"distinct_id\":\"\"}]}".to_vec(),
        "{\"batch\":[{\"event\":\"\u{0000}\u{FFFF}\",\"distinct_id\":\"\u{1F600}\"}]}"
            .as_bytes()
            .to_vec(),
        format!("{{\"batch\":[{{\"event\":\"{}\",\"distinct_id\":\"u\"}}]}}", "e".repeat(100_000))
            .into_bytes(),
        b"{\"batch\":[{\"event\":\"x\",\"distinct_id\":\"u\",\"timestamp\":\"+275760-09-13T00:00:00Z\"}],\"sent_at\":\"-262143-01-01T00:00:00Z\"}".to_vec(),
        deep.into_bytes(),
        wide.into_bytes(),
        vec![0xff, 0xfe, 0x00, 0x7b],
    ]
}
