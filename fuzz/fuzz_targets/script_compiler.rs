#![no_main]

#[path = "../support/script_compiler.rs"]
mod cases;

libfuzzer_sys::fuzz_target!(|data: &[u8]| cases::exercise(data));
