//! PuTTY key files from untrusted sources must never crash the importer.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = rooste_keys::load(text, None);
        let _ = rooste_keys::load(text, Some("rooste"));
    }
});
