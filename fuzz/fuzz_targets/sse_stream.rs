#![no_main]

#[path = "../support/sse_stream.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::exercise(data));
