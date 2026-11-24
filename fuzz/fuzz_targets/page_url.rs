//! A page URL sent by a browser: clean it as it is stored. The result never
//! grows and never keeps a fragment.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(url) = std::str::from_utf8(data) {
        let clean = openlens_event::clean_url(url);
        assert!(clean.len() <= url.len());
        assert!(!clean.contains('#'));
    }
});
