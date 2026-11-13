#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;
fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let _ = btlib::suite::parse(Path::new("fuzz.bt"), &s);
    let _ = btlib::suite::tokenize(&s);
    let _ = btlib::suite::csv_rows(&s);
    let words: Vec<String> = s.split_whitespace().map(|w| w.to_string()).collect();
    let _ = btlib::suite::parse_words(&words);
});
