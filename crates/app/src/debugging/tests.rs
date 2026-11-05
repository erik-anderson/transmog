use super::*;
#[test]
fn local_ownership_and_mapping_do_not_follow_other_windows_or_preceding_lines() {
    let manager = Debuggers::default();
    let configuration = DebugConfiguration {
        pause_on_start: false,
        persistent: false,
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    };
    let run = manager
        .create(
            "window-one",
            &configuration,
            DebugSessionInfo {
                session: String::new(),
                mode: "test".into(),
                label: "fixture".into(),
                artifact: "immutable-artifact".into(),
                source: "file:///main.ts".into(),
            },
            None,
        )
        .unwrap();
    assert!(manager.owned("window-two", run.session.id()).is_err());
    assert!(manager.owned("window-one", run.session.id()).is_ok());
    let map=br#"{"version":3,"sources":["file:///main.ts"],"sourcesContent":["const value: number = 7;"],"names":[],"mappings":"AAAA"}"#;
    let source = SourceBreakpoint {
        condition: None,
        id: 1,
        location: SourceLocation {
            source: "file:///main.ts".into(),
            line: 1,
            column: 1,
        },
    };
    assert_eq!(
        translate(std::slice::from_ref(&source), Some(map))[0]
            .location
            .source,
        "file:///transmog-script/main.js"
    );
    let mut absent = source;
    absent.location.line = 2;
    assert_eq!(translate(&[absent.clone()], Some(map))[0], absent);
    let map = decode(map).unwrap();
    assert!(same_line_token(&map, 1, 0).is_none());
    manager.release("window-one");
    assert!(run.session.is_closed());
    assert!(manager.owned("window-one", run.session.id()).is_err());
}

#[test]
fn debug_result_serialization_stops_before_materializing_a_large_value() {
    struct Count(std::sync::atomic::AtomicUsize);
    impl Serialize for Count {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq as _;
            let mut sequence = serializer.serialize_seq(Some(1_000_000))?;
            for _ in 0..1_000_000 {
                self.0.fetch_add(1, Ordering::Relaxed);
                sequence.serialize_element(&7)?;
            }
            sequence.end()
        }
    }
    let value = Count(std::sync::atomic::AtomicUsize::new(0));
    assert_eq!(limited_json(&value), (None, true));
    assert!(value.0.load(Ordering::Relaxed) < 40_000);
    assert_eq!(
        limited_json(&serde_json::json!({"ok":7})),
        (Some(serde_json::json!({"ok":7})), false)
    );
}
#[test]
fn closed_histories_are_globally_bounded_and_late_completion_cannot_restore_evicted_values() {
    let manager = Debuggers::default();
    let mut held = Vec::new();
    let config = DebugConfiguration {
        pause_on_start: false,
        persistent: false,
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    };
    for index in 0..20 {
        let run = manager
            .create(
                "history-owner",
                &config,
                DebugSessionInfo {
                    session: String::new(),
                    mode: "test".into(),
                    label: index.to_string(),
                    artifact: index.to_string(),
                    source: "file:///main.js".into(),
                },
                Some(vec![0; 1024]),
            )
            .unwrap();
        run.finish(outcome(
            "completed",
            "fixture",
            Some(serde_json::json!({"private":"ephemeral"})),
            None,
            vec![],
        ));
        assert!(run.map.lock().unwrap().is_none());
        held.push(run);
    }
    let first = &held[0];
    assert!(first.expired.load(Ordering::Acquire));
    first.record(outcome(
        "completed",
        "late",
        Some(serde_json::json!({"must_not_return":"private"})),
        None,
        vec![],
    ));
    assert_eq!(
        first.outcome.lock().unwrap().as_ref().unwrap().status,
        "expired"
    );
    assert_eq!(first.session.page(0).logs, vec![]);
    assert!(manager.0.lock().unwrap().len() <= 12);
    manager.release("history-owner");
}
