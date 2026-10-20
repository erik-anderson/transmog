//! App metadata is independent of lazy ordinary tmext resources.
use std::{
    collections::BTreeMap,
    fs::File,
    sync::{Arc, OnceLock, atomic::AtomicBool},
};
use transmog_extension_registry::{BundledExtension, Registry};
use transmog_extensions::{Manifest, Origin, Snapshot, write_package};

const SOURCE:&[u8]=b"export function visualize(){return {kind:'document',document:{title:'Bundled',status:'complete',presentation:'tree',roots:[]}};}";
fn snapshot(version: &str) -> Snapshot {
    let manifest:Manifest=serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.bundled","version":version,"name":"Bundled example",
        "visualizers":[{"id":"headers","name":"Headers","entry":"main.js","surfaces":["headers"],"inputs":{"request":{"headers":["x-example"],"body":"none"},"response":{"headers":[],"body":"none"}},"matches":[{"direction":"request","headers":[{"direction":"request","name":"x-example","test":{"op":"present"}}]}]}],
        "files":[{"path":"main.js","bytes":SOURCE.len()}]
    })).unwrap();
    Snapshot::new(
        manifest,
        BTreeMap::from([("main.js".into(), Arc::<[u8]>::from(SOURCE))]),
    )
    .unwrap()
}
fn archive(snapshot: &Snapshot) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_package(&mut bytes, snapshot, &AtomicBool::new(false)).unwrap();
    bytes
}
fn version_one() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| archive(&snapshot("1.0.0")))
}
fn version_two() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| archive(&snapshot("2.0.0")))
}
fn metadata(version: &str) -> &'static [u8] {
    Box::leak(
        serde_json::to_vec(&snapshot(version).manifest)
            .unwrap()
            .into_boxed_slice(),
    )
}
fn never_open() -> &'static [u8] {
    panic!("Metadata discovery opened bundled code")
}

#[test]
fn bundled_discovery_listing_and_enable_do_not_open_archive_or_create_install_files() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("extensions");
    let resource = BundledExtension {
        manifest: metadata("1.0.0"),
        archive: never_open,
    };
    let registry = Registry::load_with_bundled(store.clone(), None, &[resource]);
    assert!(!store.exists());
    assert_eq!(registry.status().entries[0].catalog.origin, Origin::Bundled);
    assert_eq!(registry.enabled_visualizers().len(), 1);
    assert!(registry.classification_choices().is_empty());
    registry.set_enabled("example.bundled", false).unwrap();
    registry.set_enabled("example.bundled", true).unwrap();
    assert_eq!(
        std::fs::read_dir(&store).unwrap().count(),
        1,
        "Only metadata is persisted"
    );
    assert!(registry.uninstall("example.bundled").is_err());
    assert!(registry.set_hot_reload("example.bundled", true).is_err());
}

#[test]
fn bundled_upgrade_refreshes_trusted_metadata_and_archive_while_preserving_disable_choice() {
    let root = tempfile::tempdir().unwrap();
    let first = Registry::load_with_bundled(
        root.path().into(),
        None,
        &[BundledExtension {
            manifest: metadata("1.0.0"),
            archive: version_one,
        }],
    );
    assert_eq!(
        first.snapshot("example.bundled").unwrap().manifest.version,
        "1.0.0"
    );
    first.set_enabled("example.bundled", false).unwrap();
    let next = Registry::load_with_bundled(
        root.path().into(),
        None,
        &[BundledExtension {
            manifest: metadata("2.0.0"),
            archive: version_two,
        }],
    );
    assert!(!next.status().entries[0].catalog.enabled);
    assert_eq!(next.status().entries[0].catalog.manifest.version, "2.0.0");
    assert!(next.snapshot("example.bundled").is_err());
    next.set_enabled("example.bundled", true).unwrap();
    let snapshot = next.snapshot("example.bundled").unwrap();
    assert_eq!(snapshot.manifest.version, "2.0.0");
    assert_eq!(snapshot.files["main.js"].as_ref(), SOURCE);
    let development = root.path().join("developer");
    std::fs::create_dir(&development).unwrap();
    std::fs::write(
        development.join("manifest.json"),
        serde_json::to_vec(&snapshot.manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(development.join("main.js"), SOURCE).unwrap();
    assert!(
        next.register_directory(&development, false).is_err(),
        "A development folder replaced bundled provenance"
    );
    let candidate = root.path().join("replacement.tmext");
    write_package(
        File::create(&candidate).unwrap(),
        &snapshot,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        next.install(&candidate).is_err(),
        "A user package replaced bundled provenance"
    );
    assert_eq!(next.status().entries[0].catalog.origin, Origin::Bundled);
}

#[test]
fn bundled_resources_do_not_reenable_after_corrupt_catalog_or_overwrite_recovery_data() {
    let root = tempfile::tempdir().unwrap();
    let catalog = root.path().join("catalog.json");
    std::fs::write(&catalog, b"corrupt metadata").unwrap();
    let registry = Registry::load_with_bundled(
        root.path().into(),
        None,
        &[BundledExtension {
            manifest: metadata("1.0.0"),
            archive: never_open,
        }],
    );
    assert!(registry.status().warning.is_some());
    assert!(registry.status().management_blocked);
    assert!(!registry.status().entries[0].catalog.enabled);
    assert!(registry.set_enabled("example.bundled", true).is_err());
    assert_eq!(std::fs::read(catalog).unwrap(), b"corrupt metadata");
}

#[test]
fn bundled_id_collision_preserves_user_files_and_allows_explicit_uninstall_recovery() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("extensions");
    let file = root.path().join("user.tmext");
    write_package(
        File::create(&file).unwrap(),
        &snapshot("0.9.0"),
        &AtomicBool::new(false),
    )
    .unwrap();
    Registry::load(store.clone(), None).install(&file).unwrap();
    let resource = BundledExtension {
        manifest: metadata("1.0.0"),
        archive: never_open,
    };
    let registry = Registry::load_with_bundled(store.clone(), None, &[resource]);
    assert!(registry.status().warning.is_some());
    assert!(!registry.status().management_blocked);
    assert_eq!(
        registry.status().entries[0].catalog.origin,
        Origin::Installed
    );
    assert!(!registry.status().entries[0].catalog.enabled);
    assert!(file.is_file());
    registry.uninstall("example.bundled").unwrap();
    assert!(file.is_file());
    let reopened = Registry::load_with_bundled(store, None, &[resource]);
    assert_eq!(reopened.status().entries[0].catalog.origin, Origin::Bundled);
    assert!(reopened.status().entries[0].catalog.enabled);
}

#[test]
fn bundled_missing_or_mismatched_archive_fails_only_explicit_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let mismatch = Registry::load_with_bundled(
        root.path().into(),
        None,
        &[BundledExtension {
            manifest: metadata("1.0.0"),
            archive: version_two,
        }],
    );
    assert_eq!(mismatch.enabled_visualizers().len(), 1);
    assert!(mismatch.snapshot("example.bundled").is_err());
    mismatch.set_enabled("example.bundled", true).unwrap();
    let absent = Registry::load(root.path().into(), None);
    assert!(absent.enabled_visualizers().is_empty());
    assert!(absent.snapshot("example.bundled").is_err());
}
