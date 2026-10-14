//! Builds only the pinned PKI target against boring-sys's existing crypto ABI.
use std::{env, fs, path::PathBuf};
mod build_support;

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let vendor = root.join("third_party/boringssl-pki");
    for (name, _) in env::vars_os() {
        let name = name.to_string_lossy();
        assert!(
            !build_support::replaces_native_library(&name),
            "Remove {name}: native TLS source/header/archive overrides are incompatible with the qualified PKI snapshot"
        );
    }
    let target = env::var("TARGET").unwrap();
    for option in ["PATH", "INCLUDE_PATH", "SOURCE_PATH", "ASSUME_PATCHED"] {
        for prefix in ["BORING_BSSL_", "BORING_BSSL_FIPS_"] {
            let name = format!("{prefix}{option}");
            for key in [
                name.clone(),
                format!("HOST_{name}"),
                format!("TARGET_{name}"),
                format!("{name}_{target}"),
                format!("{name}_{}", target.replace('-', "_")),
            ] {
                println!("cargo:rerun-if-env-changed={key}");
            }
        }
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(vendor.join("manifest.json")).unwrap()).unwrap();
    let expected_major = manifest["boring_version"]
        .as_str()
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    println!("cargo:rerun-if-env-changed=DEP_BORINGSSL_VERSION_MAJOR");
    assert!(
        env::var("DEP_BORINGSSL_VERSION_MAJOR").as_deref() == Ok(expected_major),
        "The resolved boring-sys major version disagrees with the PKI receipt; run the TLS dependency preflight"
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
    for source in manifest["sources"].as_array().unwrap() {
        build.file(vendor.join(source.as_str().unwrap()));
    }
    build.flag_if_supported("/EHsc");
    build.compile("transmog_pki");
}
