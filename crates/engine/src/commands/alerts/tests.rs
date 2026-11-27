use super::Settings;

#[test]
fn settings_have_defaults_and_are_checked() {
    let s = Settings::default();
    assert!(s.down && s.traffic && s.email && s.validate().is_ok());
    assert!(
        Settings {
            webhook: "http://x".into(),
            ..s.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        Settings {
            ntfy: "my topic".into(),
            ..s.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        Settings {
            ntfy: "ops-alerts".into(),
            ..s.clone()
        }
        .validate()
        .is_ok()
    );
    assert!(
        Settings {
            health_path: "healthz".into(),
            ..s.clone()
        }
        .validate()
        .is_err()
    );
    assert!(Settings { down_wait: 0, ..s }.validate().is_err());
    let partial: Settings = serde_json::from_str(r#"{"traffic": false}"#).unwrap_or_default();
    assert!(
        !partial.traffic && partial.down,
        "missing fields take the defaults"
    );
}
