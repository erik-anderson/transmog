#![no_main]
use libfuzzer_sys::fuzz_target;
#[path = "../support/capture_reader.rs"]
mod harness;

fuzz_target!(|input: &[u8]| harness::exercise(input));
