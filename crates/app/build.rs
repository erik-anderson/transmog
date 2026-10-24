//! Embeds build identity, dependency versions and bundled source receipts.

use std::{env, path::PathBuf};

#[allow(dead_code)]
#[path = "../../build-support/diagnostics.rs"]
mod diagnostics;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    diagnostics::application(&manifest_dir.join("../.."));
}
