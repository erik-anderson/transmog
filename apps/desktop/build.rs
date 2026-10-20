//! Delegates Windows resources to Tauri.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

fn mapi_bundle() {
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
            .join("../../extensions/mapi");
    for source in [
        "manifest.json",
        "tsconfig.json",
        "build.mjs",
        "typescript",
        "README.runtime.md",
        "LICENSES/Office-MIT.txt",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(source).display());
    }
    let build = std::process::Command::new("node")
        .arg(root.join("build.mjs"))
        .output()
        .expect("MAPI TypeScript build requires Node and the desktop UI's locked npm dependencies");
    assert!(
        build.status.success(),
        "MAPI TypeScript build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let metadata = std::fs::read(root.join("dist/package-manifest.json"))
        .expect("Built MAPI runtime metadata");
    let manifest =
        transmog_extensions::Manifest::parse(&metadata, false).expect("Valid pinned MAPI metadata");
    let files: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|file| {
            let path = root.join(&file.path);
            println!("cargo:rerun-if-changed={}", path.display());
            (
                file.path.clone(),
                Arc::<[u8]>::from(std::fs::read(path).expect("Declared MAPI runtime file")),
            )
        })
        .collect();
    let snapshot = transmog_extensions::Snapshot::new(manifest, files)
        .expect("MAPI runtime does not match its generated build metadata");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    transmog_extensions::write_package(
        std::fs::File::create(output.join("mapi.tmext")).expect("MAPI bundle output"),
        &snapshot,
        &AtomicBool::new(false),
    )
    .expect("Stored ZIP64 MAPI bundle");
    std::fs::write(output.join("mapi-manifest.json"), metadata).expect("MAPI metadata sidecar");
}

fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");
    mapi_bundle();

    #[cfg(windows)]
    tauri_build::build();
}
