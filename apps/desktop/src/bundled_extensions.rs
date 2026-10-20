//! Small app metadata is available without touching the ordinary tmext resource.
use transmog_app::extensions::BundledExtension;

pub(super) static RESOURCES: [BundledExtension; 1] = [BundledExtension {
    previous_ids: &["com.transmog.mapi"],
    manifest: include_bytes!(concat!(env!("OUT_DIR"), "/mapi-manifest.json")),
    archive: mapi_archive,
}];
fn mapi_archive() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/mapi.tmext"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_app::extensions::Registry;

    #[test]
    fn bundled_mapi_discovery_uses_only_generated_metadata() {
        fn no_archive() -> &'static [u8] {
            panic!("Discovery opened the MAPI archive")
        }
        let root = tempfile::tempdir().unwrap();
        let registry = Registry::load_with_bundled(
            root.path().join("extensions"),
            None,
            &[BundledExtension {
                previous_ids: &[],
                manifest: RESOURCES[0].manifest,
                archive: no_archive,
            }],
        );
        let status = registry.status();
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].catalog.manifest.id, "net.transmog.mapi");
        assert_eq!(registry.enabled_visualizers().len(), 3);
        assert_eq!(registry.classification_choices().len(), 1);
        assert!(!root.path().join("extensions").exists());
    }

    #[test]
    fn bundled_mapi_is_an_ordinary_minimal_zip64_package() {
        let root = tempfile::tempdir().unwrap();
        let registry = Registry::load_with_bundled(root.path().into(), None, &RESOURCES);
        let snapshot = registry.snapshot("net.transmog.mapi").unwrap();
        assert_eq!(snapshot.files.len(), 4);
        assert!(snapshot.files.keys().all(|name| matches!(
            name.as_str(),
            "dist/runtime/body.js"
                | "dist/runtime/headers.js"
                | "LICENSES/Office-MIT.txt"
                | "README.runtime.md"
        )));
        assert!(
            mapi_archive()
                .windows(4)
                .any(|bytes| bytes == [0x50, 0x4b, 0x06, 0x06]),
            "ZIP64 end record missing"
        );
        registry.set_enabled("net.transmog.mapi", false).unwrap();
        let reopened = Registry::load_with_bundled(root.path().into(), None, &RESOURCES);
        assert!(reopened.enabled_visualizers().is_empty());
        assert!(!reopened.status().entries[0].catalog.enabled);
    }
}
