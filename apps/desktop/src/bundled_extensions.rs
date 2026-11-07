//! Small app metadata is available without touching the ordinary tmext resource.
use transmog_app::extensions::BundledExtension;

pub(super) static RESOURCES: [BundledExtension; 2] = [
    BundledExtension {
        previous_ids: &["com.transmog.mapi"],
        manifest: include_bytes!(concat!(env!("OUT_DIR"), "/mapi-manifest.json")),
        archive: mapi_archive,
    },
    BundledExtension {
        previous_ids: &[],
        manifest: include_bytes!(concat!(env!("OUT_DIR"), "/binary-manifest.json")),
        archive: binary_archive,
    },
];
fn mapi_archive() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/mapi.tmext"))
}
fn binary_archive() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/binary.tmext"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_app::extensions::Registry;

    #[test]
    fn bundled_typescript_is_browsable_and_breakpoints_project_to_each_runtime() {
        let root = tempfile::tempdir().unwrap();
        let registry = Registry::load_with_bundled(root.path().into(), None, &RESOURCES);
        let application = transmog_app::Application::new(transmog_app::AppConfig {
            extensions: Some(registry),
            ..Default::default()
        })
        .unwrap();
        let workspace = application.open_debugger_workspace().unwrap();
        assert!(
            workspace.targets.is_empty(),
            "Browsing must not start workers"
        );
        for (package, module, original) in [
            ("mapi", "body", "body"),
            ("mapi", "headers", "headers"),
            ("binary", "protobuf", "protobuf-entry"),
            ("binary", "bond", "bond-entry"),
            ("binary", "msgpack", "msgpack-entry"),
            ("binary", "cbor", "cbor-entry"),
            ("binary", "bson", "bson-entry"),
        ] {
            let id = format!("net.transmog.{package}");
            let source = workspace
                .files
                .iter()
                .find(|file| file.package == id && file.path == format!("typescript/{original}.ts"))
                .unwrap();
            assert_eq!(source.language, "typescript");
            assert_eq!(source.state, "Unloaded");
            let text = application
                .debugger_file(&source.id, &source.artifact)
                .unwrap();
            let authored =
                std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
                    format!("../../extensions/{package}/typescript/{original}.ts"),
                ))
                .unwrap();
            assert_eq!(
                text, authored,
                "Packaged TypeScript must match its build input"
            );
            let generated = workspace
                .files
                .iter()
                .find(|file| file.package == id && file.path == format!("dist/runtime/{module}.js"))
                .unwrap();
            assert_eq!(source.counterpart.as_deref(), Some(generated.id.as_str()));
            let line = text
                .lines()
                .position(|line| line.starts_with("export function visualize("))
                .unwrap()
                + 2;
            let point = serde_json::from_value(serde_json::json!({
                "artifact": source.artifact,
                "file": source.id,
                "enabled": true,
                "anchor": text.lines().nth(line - 1).unwrap(),
                "breakpoint": {
                    "id": 1,
                    "location": {"source": source.source, "line": line, "column": 1}
                }
            }))
            .unwrap();
            application.set_workspace_breakpoints(&[point]).unwrap();
            let positions = application
                .debugger_breakpoint_positions(&generated.id, &generated.artifact)
                .unwrap();
            assert_eq!(
                positions.len(),
                1,
                "{package}/{module} breakpoint was not mapped"
            );
            assert_eq!(positions[0].1.source, generated.source);
        }
        assert!(application.debugger_workspace().unwrap().targets.is_empty());
        application.close_debugger_workspace().unwrap();
    }

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
        assert_eq!(snapshot.files.len(), 6);
        assert!(snapshot.files.keys().all(|name| matches!(
            name.as_str(),
            "dist/runtime/body.js"
                | "dist/runtime/headers.js"
                | "dist/runtime/body.js.map"
                | "dist/runtime/headers.js.map"
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
        assert_eq!(
            reopened
                .enabled_visualizers()
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            ["net.transmog.binary"; 10]
        );
        assert!(
            !reopened
                .status()
                .entries
                .iter()
                .find(|entry| entry.catalog.manifest.id == "net.transmog.mapi")
                .unwrap()
                .catalog
                .enabled
        );
    }

    #[test]
    fn bundled_binary_formats_use_the_ordinary_lazy_package_contract() {
        fn no_archive() -> &'static [u8] {
            panic!("Discovery opened the binary archive")
        }
        let root = tempfile::tempdir().unwrap();
        let registry = Registry::load_with_bundled(
            root.path().into(),
            None,
            &[BundledExtension {
                previous_ids: &[],
                manifest: RESOURCES[1].manifest,
                archive: no_archive,
            }],
        );
        assert_eq!(registry.enabled_visualizers().len(), 10);
        assert!(!root.path().join("catalog.json").exists());
        let registry = Registry::load_with_bundled(root.path().into(), None, &RESOURCES);
        let snapshot = registry.snapshot("net.transmog.binary").unwrap();
        assert_eq!(snapshot.files.len(), 11);
        assert!(snapshot.files.keys().all(|name| matches!(
            name.as_str(),
            "dist/runtime/protobuf.js"
                | "dist/runtime/bond.js"
                | "dist/runtime/msgpack.js"
                | "dist/runtime/cbor.js"
                | "dist/runtime/bson.js"
                | "dist/runtime/protobuf.js.map"
                | "dist/runtime/bond.js.map"
                | "dist/runtime/msgpack.js.map"
                | "dist/runtime/cbor.js.map"
                | "dist/runtime/bson.js.map"
                | "README.runtime.md"
        )));
        assert!(
            snapshot
                .manifest
                .visualizers
                .iter()
                .all(|viewer| viewer.automatic
                    && viewer.context.is_none()
                    && viewer.live_updates.is_none())
        );
        registry.set_enabled("net.transmog.binary", false).unwrap();
        let reopened = Registry::load_with_bundled(root.path().into(), None, &RESOURCES);
        assert!(
            reopened
                .enabled_visualizers()
                .iter()
                .all(|(id, _)| id == "net.transmog.mapi")
        );
    }
}
