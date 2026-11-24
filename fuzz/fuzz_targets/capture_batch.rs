//! A body sent to `/capture`, after decompression: parse and normalize it.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(batch) = serde_json::from_slice::<openlens_event::CaptureBatch>(data) {
        let _ = openlens_event::normalize_batch("p", batch, chrono::Utc::now());
    }
});
