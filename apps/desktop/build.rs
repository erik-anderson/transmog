//! Delegates Windows resources to Tauri.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

#[allow(dead_code)]
#[path = "../../build-support/diagnostics.rs"]
mod diagnostics;

fn extension_bundle(name: &str, sources: &[&str]) {
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
            .join("../../extensions")
            .join(name);
    for source in sources {
        println!("cargo:rerun-if-changed={}", root.join(source).display());
    }
    let build = std::process::Command::new("node")
        .arg(root.join("build.mjs"))
        .output()
        .expect("Bundled extensions require Node and the desktop UI's locked npm dependencies");
    assert!(
        build.status.success(),
        "{name} TypeScript build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let metadata = std::fs::read(root.join("dist/package-manifest.json"))
        .expect("Built extension runtime metadata");
    let manifest =
        transmog_extensions::Manifest::parse(&metadata, false).expect("Valid bundled metadata");
    let files: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|file| {
            let path = root.join(&file.path);
            println!("cargo:rerun-if-changed={}", path.display());
            (
                file.path.clone(),
                Arc::<[u8]>::from(std::fs::read(path).expect("Declared extension runtime file")),
            )
        })
        .collect();
    let snapshot = transmog_extensions::Snapshot::new(manifest, files)
        .expect("Extension runtime does not match its generated build metadata");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    transmog_extensions::write_package(
        std::fs::File::create(output.join(format!("{name}.tmext")))
            .expect("Extension bundle output"),
        &snapshot,
        &AtomicBool::new(false),
    )
    .expect("Stored ZIP64 extension bundle");
    std::fs::write(output.join(format!("{name}-manifest.json")), metadata)
        .expect("Extension metadata sidecar");
}

fn main() {
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
            .join("../..");
    diagnostics::desktop(&root);
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");
    extension_bundle(
        "mapi",
        &[
            "manifest.json",
            "tsconfig.json",
            "build.mjs",
            "typescript",
            "README.runtime.md",
            "LICENSES/Office-MIT.txt",
        ],
    );
    println!("cargo:rerun-if-changed=../../crates/extensions/sdk/transmog-extensions.d.ts");
    extension_bundle(
        "binary",
        &[
            "manifest.json",
            "tsconfig.json",
            "build.mjs",
            "typescript",
            "README.runtime.md",
        ],
    );

    #[cfg(windows)]
    tauri_build::build();
}
