use super::{DirectoryWatch, Notifications, Queue, native::parse};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
// Observer lifetime assertions require isolation from other tests in this process.
static WATCH_TEST: Mutex<()> = Mutex::new(());

fn wait(watch: &mut DirectoryWatch, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(changes) = watch.poll().unwrap()
            && (changes.overflowed || changes.names.iter().any(|item| item == name))
        {
            return;
        }
        assert!(Instant::now() < deadline, "No notification for {name}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn notification_ownership_survives_its_opening_thread_and_drop_releases_the_directory() {
    let _serial = WATCH_TEST.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let opening = source.clone();
    let mut watch = std::thread::spawn(move || DirectoryWatch::open(&opening).unwrap())
        .join()
        .unwrap();
    assert!(watch.poll().unwrap().is_none());
    std::fs::write(source.join("main.js"), b"export function visualize(){}").unwrap();
    wait(&mut watch, "main.js");
    let stopped = Instant::now();
    drop(watch);
    assert!(stopped.elapsed() < Duration::from_secs(1));
    let renamed = root.path().join("renamed");
    assert_eq!(source.parent(), Some(root.path()));
    assert_eq!(renamed.parent(), Some(root.path()));
    std::fs::rename(&source, &renamed).unwrap();
}

#[test]
fn watches_share_an_observer_and_slow_consumers_do_not_block_other_roots() {
    let _serial = WATCH_TEST.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let mut slow = DirectoryWatch::open(&first).unwrap();
    let mut other = DirectoryWatch::open(&second).unwrap();
    assert!(Arc::ptr_eq(&slow.observer, &other.observer));
    let observer = Arc::downgrade(&slow.observer);
    let filename = first.join("main.js");
    for value in 0..128 {
        std::fs::write(&filename, value.to_string()).unwrap();
    }
    std::fs::write(second.join("other.js"), b"independent").unwrap();
    wait(&mut other, "other.js");
    wait(&mut slow, "main.js");
    // After consuming the bounded backlog, atomic file saves and directory renames
    // must remain observable on the same handle.
    let nested = first.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("new.js"), b"atomic save").unwrap();
    wait(&mut slow, "nested\\new.js");
    let moved = first.join("renamed");
    assert_eq!(nested.parent(), moved.parent());
    std::fs::rename(&nested, &moved).unwrap();
    wait(&mut slow, "renamed");
    let replacement = first.join("save.tmp");
    std::fs::write(&replacement, b"replacement module").unwrap();
    assert_eq!(replacement.parent(), filename.parent());
    std::fs::rename(&replacement, &filename).unwrap();
    wait(&mut slow, "main.js");
    assert_eq!(std::fs::read(&filename).unwrap(), b"replacement module");
    drop(slow);
    std::fs::write(second.join("after.js"), b"still observed").unwrap();
    wait(&mut other, "after.js");
    drop(other);
    assert!(
        observer.upgrade().is_none(),
        "Observer retained after its last watch"
    );
}

#[test]
fn repeated_cancellation_during_writes_joins_before_closing_and_reusing_paths() {
    let _serial = WATCH_TEST.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut watches = Vec::new();
    for _ in 0..8 {
        watches.push(DirectoryWatch::open(&source).unwrap());
    }
    for value in 0..16 {
        std::fs::write(source.join("main.js"), value.to_string()).unwrap();
        drop(watches.pop());
        watches.push(DirectoryWatch::open(&source).unwrap());
    }
    drop(watches);
    let renamed = root.path().join("renamed");
    assert_eq!(source.parent(), renamed.parent());
    std::fs::rename(&source, &renamed).unwrap();
}

#[test]
fn notification_overflow_and_malformed_extents_request_revalidation() {
    assert!(parse(&[]).overflowed);
    for length in 1..12 {
        assert!(parse(&vec![0; length]).overflowed);
    }
    let mut bytes = [0_u8; 16];
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse(&bytes).overflowed);
    bytes[8..12].copy_from_slice(&3_u32.to_le_bytes());
    assert!(parse(&bytes).overflowed);
    bytes[8..12].copy_from_slice(&2_u32.to_le_bytes());
    bytes[12..14].copy_from_slice(&65_u16.to_le_bytes());
    assert_eq!(parse(&bytes).names, ["A"]);
    let mut queue = Queue::default();
    assert!(queue.push(parse(&bytes)));
    assert!(!queue.push(parse(&bytes)));
    let pending = queue.pending.take().unwrap();
    assert!(
        !pending.overflowed,
        "Coalescing duplicated names manufactured loss"
    );
    assert_eq!(pending.names, ["A"], "Slow consumer grew the pending batch");
    assert!(queue.push(parse(&bytes)));
    assert!(!queue.push(Notifications {
        names: (0..2048).map(|i| format!("field-{i}.js")).collect(),
        overflowed: false,
    }));
    let pending = queue.pending.take().unwrap();
    assert!(
        pending.overflowed,
        "Dropping excess names failed to report loss"
    );
    assert_eq!(pending.names.len(), 1024);
    bytes[..4].copy_from_slice(&4_u32.to_le_bytes());
    assert!(parse(&bytes).overflowed);
    bytes[..4].fill(0);
    bytes[12..14].copy_from_slice(&0xD800_u16.to_le_bytes());
    assert!(
        parse(&bytes).overflowed,
        "Malformed UTF-16 silently hid a source edit"
    );
}
