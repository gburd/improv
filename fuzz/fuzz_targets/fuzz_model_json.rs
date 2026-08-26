//! Fuzz `serde_json::from_slice::<Model>` with arbitrary bytes.
//! Property: deserialization never panics (a parse error is fine).
#![no_main]

use improv_core_model::Model;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Model>(data);
});
