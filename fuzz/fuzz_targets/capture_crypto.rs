#![no_main]
use libfuzzer_sys::fuzz_target;
#[path = "../support/capture_crypto.rs"]
mod harness;

fuzz_target!(|input: &[u8]| harness::exercise(input));
