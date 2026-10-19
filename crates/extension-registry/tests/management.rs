//! Metadata-only installation, developer-source boundaries and atomic management behavior.
use std::{
    collections::BTreeMap,
    fs::File,
    sync::{Arc, atomic::AtomicBool},
};
use transmog_extension_registry::Registry;
use transmog_extensions::{Manifest, Snapshot, write_package};

fn manifest() -> Manifest {
    serde_json::from_value(serde_json::json!({
    "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.headers","version":"1.0.0","name":"Header viewer",
    "visualizers":[{"id":"headers","name":"Headers","entry":"src/main.js","surfaces":["headers"],
        "inputs":{"request":{"headers":["x-example"],"body":"none"},"response":{"headers":[],"body":"none"}},
        "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-example","test":{"op":"present"}}]}]}],
    "files":[{"path":"src/main.js","bytes":0}]
})).unwrap()
}
fn package(path: &std::path::Path) {
    package_version(path, "1.0.0");
}
fn package_version(path: &std::path::Path, version: &str) {
    let mut m = manifest();
    m.version = version.into();
    let source=b"export function visualize(){return {kind:'document',document:{title:'Headers',status:'complete',presentation:'tree',roots:[]}};}";
    m.files[0].bytes = source.len() as u64;
    let snapshot = Snapshot::new(
        m,
        BTreeMap::from([("src/main.js".into(), Arc::<[u8]>::from(source.as_slice()))]),
    )
    .unwrap();
    write_package(
        File::create(path).unwrap(),
        &snapshot,
        &AtomicBool::new(false),
    )
    .unwrap();
}
#[test]
fn install_persists_inert_metadata_and_enable_never_requires_a_runtime() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.tmext");
    package(&source);
    let store = root.path().join("installed");
    let registry = Registry::load(store.clone(), None);
    registry.install(&source).unwrap();
    assert_eq!(registry.status().entries.len(), 1);
    registry.set_enabled("example.headers", false).unwrap();
    assert!(!registry.status().entries[0].catalog.enabled);
    let reopened = Registry::load(store, None);
    assert!(!reopened.status().entries[0].catalog.enabled);
    reopened.set_enabled("example.headers", true).unwrap();
    assert!(reopened.status().entries[0].catalog.enabled);
    assert!(
        reopened
            .activate("owner", "example.headers", "headers")
            .is_err()
    );
    reopened.uninstall("example.headers").unwrap();
    assert!(reopened.status().entries.is_empty());
    assert!(source.is_file());
}
#[test]
fn developer_registration_and_listing_do_not_read_code_and_uninstall_keeps_sources() {
    let root = tempfile::tempdir().unwrap();
    let sources = root.path().join("developer");
    std::fs::create_dir_all(&sources).unwrap();
    std::fs::write(
        sources.join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    let registry = Registry::load(root.path().join("registry"), None);
    registry.register_directory(&sources, true).unwrap();
    assert!(registry.status().entries[0].hot_reload);
    assert!(registry.snapshot("example.headers").is_err());
    std::fs::create_dir(sources.join("src")).unwrap();
    std::fs::write(sources.join("src/main.js"), b"first revision").unwrap();
    let first = registry.snapshot("example.headers").unwrap();
    std::fs::write(
        sources.join("src/main.js"),
        b"a different source length without manually editing the manifest",
    )
    .unwrap();
    let second = registry.snapshot("example.headers").unwrap();
    assert_ne!(first.digest, second.digest);
    registry.uninstall("example.headers").unwrap();
    assert!(sources.join("src/main.js").is_file());
}
#[test]
fn catalog_failure_preserves_enabled_state_and_corruption_does_not_break_construction() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.tmext");
    package(&source);
    let store = root.path().join("registry");
    let registry = Registry::load(store.clone(), None);
    registry.install(&source).unwrap();
    std::fs::remove_file(store.join("catalog.json")).unwrap();
    std::fs::create_dir(store.join("catalog.json")).unwrap();
    assert!(registry.set_enabled("example.headers", false).is_err());
    assert!(registry.status().entries[0].catalog.enabled);
    let recovered = Registry::load(store, None);
    assert!(recovered.status().warning.is_some());
}
#[test]
fn different_provenance_cannot_silently_override_an_installed_id() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.tmext");
    package(&source);
    let registry = Registry::load(root.path().join("registry"), None);
    registry.install(&source).unwrap();
    let dev = root.path().join("developer");
    std::fs::create_dir(&dev).unwrap();
    std::fs::write(
        dev.join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    assert!(registry.register_directory(&dev, true).is_err());
    assert!(registry.status().entries[0].directory.is_none());
}

#[test]
fn failed_install_removes_candidate_and_successful_update_removes_old_revision() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.tmext");
    package(&source);
    let store = root.path().join("registry");
    let registry = Registry::load(store.clone(), None);
    let first = registry.install(&source).unwrap();
    let old = store.join(format!("example.headers.{}.tmext", first.catalog.digest));
    package_version(&source, "2.0.0");
    // A metadata rename failure must not leave a new immutable revision behind.
    std::fs::remove_file(store.join("catalog.json")).unwrap();
    std::fs::create_dir(store.join("catalog.json")).unwrap();
    assert!(registry.install(&source).is_err());
    assert_eq!(
        registry.status().entries[0].catalog.digest,
        first.catalog.digest
    );
    let packages = || {
        std::fs::read_dir(&store)
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e.path().extension().is_some_and(|e| e == "tmext"))
            .count()
    };
    assert_eq!(packages(), 1);
    std::fs::remove_dir(store.join("catalog.json")).unwrap();
    let second = registry.install(&source).unwrap();
    assert_ne!(first.catalog.digest, second.catalog.digest);
    assert!(!old.exists());
    assert_eq!(packages(), 1);
    registry.uninstall("example.headers").unwrap();
    assert_eq!(packages(), 0);
    assert!(source.exists());
}

#[cfg(windows)]
#[test]
fn developer_root_and_source_junctions_cannot_escape_to_other_directories() {
    let root = tempfile::tempdir().unwrap();
    let external = root.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(
        external.join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    std::fs::write(external.join("main.js"), b"must not read").unwrap();
    let link = root.path().join("link");
    let make_junction = |link: &std::path::Path| {
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(&external)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction fixture failed");
    };
    make_junction(&link);
    let registry = Registry::load(root.path().join("registry"), None);
    assert!(registry.register_directory(&link, false).is_err());
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(
        source.join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    make_junction(&source.join("src"));
    registry.register_directory(&source, false).unwrap();
    assert!(registry.snapshot("example.headers").is_err());
    // Remove junctions explicitly so cleanup never traverses the external fixture.
    std::fs::remove_dir(source.join("src")).unwrap();
    std::fs::remove_dir(link).unwrap();
    assert!(external.join("main.js").exists());
}
