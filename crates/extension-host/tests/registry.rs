//! Management uses the real sandbox and releases resources across window owners.
#![cfg(windows)]
mod support;
#[path = "support/worker_tests.rs"]
mod worker_tests;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use transmog_extension_registry::{InstanceResources, Registry};
use transmog_extension_supervisor::HostConfig;
use transmog_extensions::{Invocation, Manifest, VisualizationInput, VisualizationStep};
use worker_tests::WorkerTest;

const SOURCE: &str = "export function visualize(){return {kind:'document',document:{title:'View',status:'complete',presentation:'tree',roots:[]}};}";
fn manifest(package: &str) -> Manifest {
    serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":package,"version":"1.0.0","name":"Example",
        "visualizers":[{"id":"view","name":"View","entry":"main.js","surfaces":["headers"],
            "inputs":{"request":{"headers":["x-kind"],"body":"none"},"response":{"headers":[],"body":"none"}},
            "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-kind","test":{"op":"present"}}]}]}],
        "files":[{"path":"main.js","bytes":0}]
    })).unwrap()
}
fn setup(package: &str) -> (tempfile::TempDir, Registry, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let sources = root.path().join("sources");
    std::fs::create_dir(&sources).unwrap();
    write_manifest(&sources, &manifest(package));
    std::fs::write(sources.join("main.js"), SOURCE).unwrap();
    let registry = Registry::load(
        root.path().join("installed"),
        Some(HostConfig {
            executable: support::host_executable().into(),
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
fn invocation(instance: &str) -> Invocation {
    let input: VisualizationInput = serde_json::from_value(serde_json::json!({
        "instanceId":instance,"exchangeId":"selected","requestNumber":"1","generation":"1","direction":"request",
        "request":{"evidenceId":"request-head","headers":[],"method":"GET","path":"/"}
    })).unwrap();
    Invocation::Visualize {
        input: Box::new(input),
    }
}

#[tokio::test]
async fn packaged_current_sources_run_in_a_new_sandbox_without_replacing_active_development() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (root, registry, sources) = setup(&package);
    let active = registry.activate("development", &package, "view").unwrap();
    let old_worker = registry.runner("development", &active.id).unwrap();
    std::fs::write(
        sources.join("main.js"),
        SOURCE.replace("title:'View'", "title:'Packaged'"),
    )
    .unwrap();
    let output = root.path().join("saved.tmext");
    registry.package_directory(&package, &output).unwrap();
    assert!(!old_worker.is_stopped());
    let VisualizationStep::Document { document } =
        old_worker.invoke(invocation(&active.id)).await.unwrap()
    else {
        panic!("Expected original document")
    };
    assert_eq!(document.title, "View");
    let recipient = Registry::load(
        root.path().join("recipient"),
        Some(HostConfig {
            executable: support::host_executable().into(),
        }),
    );
    recipient.install(&output).unwrap();
    let imported = recipient.activate("recipient", &package, "view").unwrap();
    assert_ne!(active.digest, imported.digest);
    assert!(!imported.development);
    let VisualizationStep::Document { document } = recipient
        .runner("recipient", &imported.id)
        .unwrap()
        .invoke(invocation(&imported.id))
        .await
        .unwrap()
    else {
        panic!("Expected packaged document")
    };
    assert_eq!(document.title, "Packaged");
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
#[tokio::test]
async fn disabling_from_another_window_stops_workers_and_releases_retained_resources() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, _) = setup(&package);
    let first = registry.activate("window-one", &package, "view").unwrap();
    let second = registry.activate("window-two", &package, "view").unwrap();
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
    registry.clone().set_enabled(&package, false).unwrap();
    assert!(worker_one.is_stopped() && worker_two.is_stopped());
    assert!(resources.stopped.load(Ordering::SeqCst));
    assert!(resources.data.lock().unwrap().is_none());
    assert!(
        registry
            .resources::<Resources>("window-one", &first.id)
            .is_err()
    );
    registry.set_enabled(&package, true).unwrap();
    assert!(worker_one.is_stopped() && worker_two.is_stopped());
}
#[tokio::test]
async fn source_declarations_publish_only_after_a_validated_document() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    let mut changed = manifest(&package);
    changed.visualizers[0]
        .inputs
        .request
        .headers
        .push("x-extra".into());
    write_manifest(&sources, &changed);
    std::fs::write(sources.join("main.js"), "this is invalid JavaScript!").unwrap();
    let before = registry.status().entries[0].catalog.clone();
    assert!(registry.activate("window", &package, "view").is_err());
    assert_eq!(registry.status().entries[0].catalog, before);
    std::fs::write(sources.join("main.js"), SOURCE).unwrap();
    let instance = registry.activate("window", &package, "view").unwrap();
    assert!(
        instance
            .declaration
            .inputs
            .request
            .permits_header("x-extra")
    );
    let runner = registry.runner("window", &instance.id).unwrap();
    assert!(!runner.has_validated_document());
    assert!(registry.commit_document("window", &instance.id).is_err());
    assert_eq!(registry.status().entries[0].catalog, before);
    assert!(matches!(
        runner.invoke(invocation(&instance.id)).await.unwrap(),
        VisualizationStep::Document { .. }
    ));
    assert!(runner.has_validated_document());
    assert!(
        registry
            .commit_document_guarded("window", &instance.id, || {
                Err::<(), _>(transmog_extensions::Error::AccessDenied)
            })
            .is_err()
    );
    assert_eq!(
        registry.status().entries[0].catalog,
        before,
        "A failed lifecycle guard published metadata"
    );
    assert!(
        registry
            .commit_document("foreign-window", &instance.id)
            .is_err()
    );
    assert!(registry.commit_document("window", &instance.id).unwrap());
    assert!(
        !registry.commit_document("window", &instance.id).unwrap(),
        "Publication is idempotent"
    );
    let after = registry.status().entries[0].catalog.clone();
    assert_eq!(after.digest, instance.digest);
    assert!(
        after.manifest.visualizers[0]
            .inputs
            .request
            .permits_header("x-extra")
    );
    registry.uninstall(&package).unwrap();
    assert!(runner.is_stopped());
    assert!(sources.join("main.js").is_file());
}

#[tokio::test]
async fn failed_or_invalid_documents_preserve_catalog_declarations() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    let before = registry.status().entries[0].catalog.clone();
    let mut changed = manifest(&package);
    changed.visualizers[0]
        .inputs
        .request
        .headers
        .push("x-extra".into());
    write_manifest(&sources, &changed);
    for source in [
        "export function visualize(){throw new Error('failed parser');}",
        "export function visualize(){return {kind:'document',document:{title:'Invalid',status:'complete',presentation:'tree',roots:[{id:'broken'}]}};}",
    ] {
        std::fs::write(sources.join("main.js"), source).unwrap();
        let instance = registry.activate("window", &package, "view").unwrap();
        let runner = registry.runner("window", &instance.id).unwrap();
        assert!(runner.invoke(invocation(&instance.id)).await.is_err());
        assert!(!runner.has_validated_document());
        assert!(registry.commit_document("window", &instance.id).is_err());
        assert_eq!(registry.status().entries[0].catalog, before);
        registry.deactivate("window", &instance.id);
    }
}

#[tokio::test]
async fn a_newer_published_revision_rejects_an_older_windows_staged_document() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    let mut changed = manifest(&package);
    changed.visualizers[0]
        .inputs
        .request
        .headers
        .push("x-older".into());
    write_manifest(&sources, &changed);
    let older = registry.activate("older", &package, "view").unwrap();
    changed.visualizers[0]
        .inputs
        .request
        .headers
        .push("x-newer".into());
    write_manifest(&sources, &changed);
    let newer = registry.activate("newer", &package, "view").unwrap();
    for (owner, instance) in [("older", &older), ("newer", &newer)] {
        let runner = registry.runner(owner, &instance.id).unwrap();
        assert!(matches!(
            runner.invoke(invocation(&instance.id)).await.unwrap(),
            VisualizationStep::Document { .. }
        ));
    }
    assert!(registry.commit_document("newer", &newer.id).unwrap());
    assert!(registry.commit_document("older", &older.id).is_err());
    let after = registry.status().entries[0].catalog.clone();
    assert_eq!(after.digest, newer.digest);
    assert!(
        after.manifest.visualizers[0]
            .inputs
            .request
            .permits_header("x-newer")
    );
    registry.deactivate("newer", &newer.id);
    assert!(registry.commit_document("newer", &newer.id).is_err());
}

#[tokio::test]
async fn disabling_cancels_pending_initialization_as_well_as_ready_instances() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    std::fs::write(sources.join("main.js"), format!("while(true){{}} {SOURCE}")).unwrap();
    let starting = registry.clone();
    let worker_package = package.clone();
    let worker = std::thread::spawn(move || starting.activate("window", &worker_package, "view"));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let stopped = std::time::Instant::now();
    registry.set_enabled(&package, false).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(stopped.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test]
async fn two_contributions_from_the_same_development_revision_can_initialize_concurrently() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    let mut manifest = manifest(&package);
    let mut second = manifest.visualizers[0].clone();
    second.id = "view-two".into();
    manifest.visualizers.push(second);
    write_manifest(&sources, &manifest);
    registry.register_directory(&sources, false).unwrap();
    let other = registry.clone();
    let worker_package = package.clone();
    let first = std::thread::spawn(move || other.activate("one", &worker_package, "view").unwrap());
    let second = registry.activate("two", &package, "view-two").unwrap();
    let first = first.join().unwrap();
    assert_eq!(first.digest, second.digest);
    assert_ne!(first.id, second.id);
    let first_worker = registry.runner("one", &first.id).unwrap();
    let second_worker = registry.runner("two", &second.id).unwrap();
    registry.deactivate_owner("one");
    assert!(first_worker.is_stopped());
    assert!(!second_worker.is_stopped());
}

#[tokio::test]
async fn source_notifications_filter_unlisted_edits_and_never_read_or_execute_changed_scripts() {
    use std::{
        io::Write as _,
        os::windows::fs::OpenOptionsExt as _,
        time::{Duration, Instant},
    };
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    let instance = registry.activate("window", &package, "view").unwrap();
    let status = registry.reload_status("window", &instance.id).unwrap();
    assert!(status.available && status.enabled && !status.dirty && status.error.is_none());
    assert!(registry.reload_status("foreign", &instance.id).is_err());
    std::fs::write(sources.join("unused.js"), b"unused source").unwrap();
    let until = Instant::now() + Duration::from_millis(400);
    while Instant::now() < until {
        assert!(
            !registry
                .reload_status("window", &instance.id)
                .unwrap()
                .dirty
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // An exclusive writer prevents any reader from opening this file. Notifications
    // must still work, and the existing immutable worker must keep executing old code.
    let mut exclusive = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .share_mode(0)
        .open(sources.join("main.js"))
        .unwrap();
    exclusive.write_all(b"throw new Error('Polling must not execute edited modules'); export function visualize(){}").unwrap();
    exclusive.sync_all().unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let status = registry.reload_status("window", &instance.id).unwrap();
        assert!(status.error.is_none());
        if status.dirty {
            break;
        }
        assert!(
            Instant::now() < until,
            "Declared script change was not observed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let runner = registry.runner("window", &instance.id).unwrap();
    assert!(matches!(
        runner.invoke(invocation(&instance.id)).await.unwrap(),
        VisualizationStep::Document { .. }
    ));
    registry.acknowledge_reload("window", &instance.id);
    assert!(
        !registry
            .reload_status("window", &instance.id)
            .unwrap()
            .dirty
    );
    drop(exclusive);
    registry.set_hot_reload(&package, false).unwrap();
    assert!(
        !registry
            .reload_status("window", &instance.id)
            .unwrap()
            .enabled
    );
    let moved = sources.with_file_name("moved-sources");
    assert_eq!(sources.parent(), moved.parent());
    std::fs::rename(&sources, &moved).unwrap();
    assert!(
        !runner.is_stopped(),
        "Disabling observation stopped the visible parser"
    );
    std::fs::rename(&moved, &sources).unwrap();
    registry.set_hot_reload(&package, true).unwrap();
    assert!(
        registry
            .reload_status("window", &instance.id)
            .unwrap()
            .dirty,
        "Late observation lost edits before registration"
    );
    registry.deactivate("window", &instance.id);
    assert!(registry.reload_status("window", &instance.id).is_err());
    std::fs::rename(&sources, &moved).unwrap();
}

#[tokio::test]
async fn disabling_hot_reload_during_initialization_cannot_register_a_late_watcher() {
    use std::sync::{atomic::AtomicUsize, mpsc};
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    let (ready, opened) = mpsc::sync_channel(1);
    let (release, proceed) = mpsc::sync_channel(1);
    let initializing = registry.clone();
    let worker_package = package.clone();
    let worker = std::thread::spawn(move || {
        let calls = AtomicUsize::new(0);
        initializing.activate_cancellable("window", &worker_package, "view", || {
            // The first check precedes source observation; the second occurs in
            // worker startup, after the watch and immutable snapshot exist.
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                ready.send(()).unwrap();
                proceed.recv().unwrap();
            }
            false
        })
    });
    opened
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    registry.set_hot_reload(&package, false).unwrap();
    release.send(()).unwrap();
    let instance = worker.join().unwrap().unwrap();
    assert!(
        !registry
            .reload_status("window", &instance.id)
            .unwrap()
            .enabled
    );
    assert!(
        !registry
            .runner("window", &instance.id)
            .unwrap()
            .is_stopped()
    );
    let moved = sources.with_file_name("moved-sources");
    assert_eq!(sources.parent(), moved.parent());
    std::fs::rename(&sources, &moved).unwrap();
}

#[tokio::test]
async fn settings_reload_requests_reach_all_owners_without_hot_reload_or_starting_workers() {
    let case = WorkerTest::acquire(1).await;
    let package = case.package().to_owned();
    let (_root, registry, sources) = setup(&package);
    registry.set_hot_reload(&package, false).unwrap();
    assert_eq!(
        registry.request_reload(&package).unwrap().active_instances,
        0
    );
    let first = registry.activate("one", &package, "view").unwrap();
    let second = registry.activate("two", &package, "view").unwrap();
    assert!(
        registry
            .reload_status("one", &first.id)
            .unwrap()
            .requested
            .is_none()
    );
    // Explicit scheduling is metadata-only, even with an unreadable changed entry.
    let mut exclusive = std::fs::OpenOptions::new();
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut exclusive, 0);
    let exclusive = exclusive.write(true).open(sources.join("main.js")).unwrap();
    assert_eq!(
        registry
            .clone()
            .request_reload(&package)
            .unwrap()
            .active_instances,
        2
    );
    assert!(
        registry
            .reload_status("one", &first.id)
            .unwrap()
            .requested
            .is_some()
    );
    assert!(
        registry
            .reload_status("two", &second.id)
            .unwrap()
            .requested
            .is_some()
    );
    registry.acknowledge_reload("one", &first.id);
    assert!(
        registry
            .reload_status("one", &first.id)
            .unwrap()
            .requested
            .is_none()
    );
    assert!(
        registry
            .reload_status("two", &second.id)
            .unwrap()
            .requested
            .is_some()
    );
    drop(exclusive);
    let fresh = registry.activate("three", &package, "view").unwrap();
    assert!(
        registry
            .reload_status("three", &fresh.id)
            .unwrap()
            .requested
            .is_none()
    );
    registry.request_reload(&package).unwrap();
    assert!(
        registry
            .reload_status("three", &fresh.id)
            .unwrap()
            .requested
            .is_some()
    );
    registry.set_enabled(&package, false).unwrap();
    assert!(registry.request_reload(&package).is_err());
    assert!(registry.request_reload("not.installed").is_err());
}
