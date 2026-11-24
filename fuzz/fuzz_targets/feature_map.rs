//! A feature map pushed by an agent: parse it, then validate what parsed.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(src) = std::str::from_utf8(data) {
        let parsed = openlens_featuremap::parse(src);
        if let Some(map) = parsed.map.as_ref() {
            let _ = openlens_featuremap::validate(map);
        }
    }
});
