#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let (name, contents) = text.split_once('\n').unwrap_or((text, ""));
        let _ = rooste_core::import::putty::parse_session_file(name, contents);
    }
});
