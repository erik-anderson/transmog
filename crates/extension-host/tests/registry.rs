//! Management uses the real sandbox and releases resources across window owners.
#![cfg(windows)]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use transmog_extension_registry::{InstanceResources, Registry};
use transmog_extension_supervisor::HostConfig;
use transmog_extensions::Manifest;

const SOURCE: &str = "export function visualize(){return {kind:'document',document:{title:'View',status:'complete',presentation:'tree',roots:[]}};}";
fn manifest() -> Manifest {
    serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.registry","version":"1.0.0","name":"Example",
        "visualizers":[{"id":"view","name":"View","entry":"main.js","surfaces":["headers"],
            "inputs":{"request":{"headers":["x-kind"],"body":"none"},"response":{"headers":[],"body":"none"}},
            "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-kind","test":{"op":"present"}}]}]}],
        "files":[{"path":"main.js","bytes":0}]
    })).unwrap()
}
fn setup() -> (tempfile::TempDir, Registry, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let sources = root.path().join("sources");
    std::fs::create_dir(&sources).unwrap();
    write_manifest(&sources, &manifest());
    std::fs::write(sources.join("main.js"), SOURCE).unwrap();
    let registry = Registry::load(
        root.path().join("installed"),
        Some(HostConfig {
            executable: env!("CARGO_BIN_EXE_transmog-extension-host").into(),
        }),
    );
    registry.register_directory(&sources, true).unwrap();
    (root, registry, sources)
}
fn write_manifest(directory: &std::path::Path, manifest: &Manifest) {
    std::fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}
struct Resources {
    stopped: AtomicBool,
    data: Mutex<Option<Vec<u8>>>,
}
impl InstanceResources for Resources {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.data.lock().unwrap().take();
    }
}
impl Resources {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            stopped: AtomicBool::new(false),
            data: Mutex::new(Some(vec![1; 1024])),
        })
    }
}
#[test]
fn disabling_from_another_window_stops_workers_and_releases_retained_resources() {
    let (_root, registry, _) = setup();
    let first = registry
        .activate("window-one", "example.registry", "view")
        .unwrap();
    let second = registry
        .activate("window-two", "example.registry", "view")
        .unwrap();
    let resources = Resources::new();
    registry
        .attach_resources("window-one", &first.id, Arc::clone(&resources))
        .unwrap();
    assert!(
        registry
            .resources::<Resources>("window-two", &first.id)
            .is_err()
    );
    assert!(registry.runner("window-two", &first.id).is_err());
    let worker_one = registry.runner("window-one", &first.id).unwrap();
    let worker_two = registry.runner("window-two", &second.id).unwrap();
    registry.deactivate("window-one", &second.id);
    assert!(!worker_two.is_stopped());
    registry
        .clone()
        .set_enabled("example.registry", false)
        .unwrap();
    assert!(worker_one.is_stopped() && worker_two.is_stopped());
    assert!(resources.stopped.load(Ordering::SeqCst));
    assert!(resources.data.lock().unwrap().is_none());
    assert!(
        registry
            .resources::<Resources>("window-one", &first.id)
            .is_err()
    );
    registry.set_enabled("example.registry", true).unwrap();
    assert!(worker_one.is_stopped() && worker_two.is_stopped());
}
#[test]
fn source_declarations_publish_only_after_successful_worker_initialization() {
    let (_root, registry, sources) = setup();
    let mut changed = manifest();
    changed.visualizers[0]
        .inputs
        .request
        .headers
        .push("x-extra".into());
    write_manifest(&sources, &changed);
    std::fs::write(sources.join("main.js"), "this is invalid JavaScript!").unwrap();
    let before = registry.status().entries[0].catalog.clone();
    assert!(
        registry
            .activate("window", "example.registry", "view")
            .is_err()
    );
    assert_eq!(registry.status().entries[0].catalog, before);
    std::fs::write(sources.join("main.js"), SOURCE).unwrap();
    let instance = registry
        .activate("window", "example.registry", "view")
        .unwrap();
    assert!(
        instance
            .declaration
            .inputs
            .request
            .permits_header("x-extra")
    );
    let after = registry.status().entries[0].catalog.clone();
    assert_eq!(after.digest, instance.digest);
    assert!(
        after.manifest.visualizers[0]
            .inputs
            .request
            .permits_header("x-extra")
    );
    let runner = registry.runner("window", &instance.id).unwrap();
    registry.uninstall("example.registry").unwrap();
    assert!(runner.is_stopped());
    assert!(sources.join("main.js").is_file());
}

#[test]
fn disabling_cancels_pending_initialization_as_well_as_ready_instances() {
    let (_root, registry, sources) = setup();
    std::fs::write(sources.join("main.js"), format!("while(true){{}} {SOURCE}")).unwrap();
    let starting = registry.clone();
    let worker =
        std::thread::spawn(move || starting.activate("window", "example.registry", "view"));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let stopped = std::time::Instant::now();
    registry.set_enabled("example.registry", false).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(stopped.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn two_contributions_from_the_same_development_revision_can_initialize_concurrently() {
    let (_root, registry, sources) = setup();
    let mut manifest = manifest();
    let mut second = manifest.visualizers[0].clone();
    second.id = "view-two".into();
    manifest.visualizers.push(second);
    write_manifest(&sources, &manifest);
    registry.register_directory(&sources, false).unwrap();
    let other = registry.clone();
    let first =
        std::thread::spawn(move || other.activate("one", "example.registry", "view").unwrap());
    let second = registry
        .activate("two", "example.registry", "view-two")
        .unwrap();
    let first = first.join().unwrap();
    assert_eq!(first.digest, second.digest);
    assert_ne!(first.id, second.id);
    let first_worker = registry.runner("one", &first.id).unwrap();
    let second_worker = registry.runner("two", &second.id).unwrap();
    registry.deactivate_owner("one");
    assert!(first_worker.is_stopped());
    assert!(!second_worker.is_stopped());
}
