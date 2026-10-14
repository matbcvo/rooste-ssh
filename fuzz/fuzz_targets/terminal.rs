//! Arbitrary output from a server must never crash the terminal.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rooste_term::Terminal;

fuzz_target!(|data: &[u8]| {
    let mut term = Terminal::new(40, 12);
    // Split the input so state carries across chunks, as with real reads.
    for chunk in data.chunks(97) {
        term.advance(chunk);
    }
    term.resize(23, 7, 8, 16);
    let _ = term.screen();
    let _ = term.take_events();
    let _ = term.selection_text();
});
