//! Compiles the generated glue into the SDK's private Rust static library.
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
