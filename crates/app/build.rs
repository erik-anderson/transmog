//! Embeds the qualified TLS dependency version in support diagnostics.

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let metadata_path = manifest_dir.join("../../third_party/boringssl-pki/manifest.json");
    println!("cargo:rerun-if-changed={}", metadata_path.display());
    let metadata: serde_json::Value = serde_json::from_slice(
        &fs::read(&metadata_path).expect("read qualified TLS dependency metadata"),
    )
    .expect("parse qualified TLS dependency metadata");
    let version = metadata["boring_version"]
        .as_str()
        .expect("TLS dependency metadata provides boring_version");
    println!("cargo:rustc-env=TRANSMOG_BORING_VERSION={version}");
}
