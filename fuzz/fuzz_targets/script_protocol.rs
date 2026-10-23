#![no_main]

#[path = "../support/script_protocol.rs"]
mod cases;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cases::check(data);
});
