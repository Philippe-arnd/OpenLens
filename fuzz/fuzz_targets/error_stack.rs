//! An error stack sent by a browser: split it into frames.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(stack) = std::str::from_utf8(data) {
        let _ = openlens_errors::frames(stack);
    }
});
