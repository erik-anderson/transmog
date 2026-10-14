//! Builds only the pinned PKI target against boring-sys's existing crypto ABI.
use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let vendor = root.join("third_party/boringssl-pki");
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        cargo
            .lines()
            .any(|line| line.starts_with("boring-sys = { git =")
                && line.contains("rev = \"9c57211afd4e99479e4fbba73cd603128726275b\"")),
        "Update the PKI snapshot together with boring-sys"
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("Cargo.toml").display()
    );
    println!("cargo:rerun-if-changed={}", vendor.display());
    println!("cargo:rerun-if-changed=native/verifier.cc");
    println!("cargo:rerun-if-changed=native/security.h");
    println!("cargo:rerun-if-changed=../../third_party/chromium-certificate-policy");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .include(&vendor)
        .include(vendor.join("include"))
        .include(root.join("third_party/chromium-certificate-policy"))
        .file("native/verifier.cc");
    // Compile precisely the pinned upstream target, not arbitrary files added
    // to the vendor directory. The portable and release gates check its hashes.
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(vendor.join("manifest.json")).unwrap()).unwrap();
    for source in manifest["sources"].as_array().unwrap() {
        build.file(vendor.join(source.as_str().unwrap()));
    }
    build.flag_if_supported("/EHsc");
    build.compile("transmog_pki");
}
