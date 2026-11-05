use super::*;

#[test]
fn skipped_pauses_apply_before_initialization_without_discarding_breakpoint_intent() {
    let (_commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        let mut config = options(vec![SourceBreakpoint {
            id: 1,
            condition: None,
            log_expression: Some("'initialization'".into()),
            location: SourceLocation {
                source: SOURCE.into(),
                line: 1,
                column: 1,
            },
        }]);
        config.skip_pauses = true;
        config.pause_on_start = true;
        config.exceptions = ExceptionPolicy::All;
        runtime
            .enable_debugger(
                config,
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime
            .evaluate_modules(
                &entry,
                BTreeMap::from([(
                    entry.clone(),
                    "globalThis.value=2;debugger;try{throw Error('caught')}catch{}".into(),
                )]),
            )
            .unwrap();
        assert_eq!(
            runtime
                .execute_string("invoke", "debugger; String(value)", 32)
                .unwrap(),
            "2"
        );
    });
    engine.join().unwrap();
    let events = events.try_iter().collect::<Vec<_>>();
    assert!(!events.iter().any(|event| matches!(
        event.event,
        DebugEventKind::Paused { .. } | DebugEventKind::Logpoint { .. }
    )));
    assert!(events.iter().any(|event| matches!(&event.event, DebugEventKind::Sources { breakpoints, .. } if breakpoints.iter().any(|point| point.id == 1 && point.resolved.is_some()))));
}

#[test]
fn conditional_logpoints_report_values_and_never_pause_or_mutate_the_callback() {
    for expression in ["'value ' + input", "++touches"] {
        let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
        let (output, events) = mpsc::sync_channel(32);
        let config = options(vec![SourceBreakpoint {
            id: 1,
            condition: Some("input === 2".into()),
            log_expression: Some(expression.into()),
            location: SourceLocation {
                source: SOURCE.into(),
                line: 4,
                column: 1,
            },
        }]);
        let engine = std::thread::spawn(move || {
            let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
            runtime
                .enable_debugger(
                    config,
                    DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
                )
                .unwrap();
            let entry = url::Url::parse(SOURCE).unwrap();
            runtime.evaluate_modules(&entry, BTreeMap::from([(entry.clone(), "globalThis.touches=0;\nexport function run(input){\n const local=input;\n return local;\n}\nglobalThis.run=run;".into())])).unwrap();
            assert_eq!(
                runtime
                    .execute_string("invoke", "`${run(1)}:${run(2)}:${touches}`", 32)
                    .unwrap(),
                "1:2:0"
            );
        });
        let output = event(&events, |event| {
            matches!(event.event, DebugEventKind::Logpoint { .. })
        });
        let DebugEventKind::Logpoint {
            text,
            thrown,
            location,
            ..
        } = output.event
        else {
            unreachable!()
        };
        assert_eq!(location.unwrap().source, SOURCE);
        assert_eq!(thrown, expression == "++touches");
        if !thrown {
            assert_eq!(text, "value 2");
        }
        engine.join().unwrap();
        assert!(
            !events
                .try_iter()
                .any(|event| matches!(event.event, DebugEventKind::Paused { .. }))
        );
        drop(commands);
    }
}

#[test]
fn logpoint_floods_are_reported_without_stopping_execution() {
    let (_commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![SourceBreakpoint {
                    id: 1,
                    condition: None,
                    log_expression: Some("input".into()),
                    location: SourceLocation {
                        source: SOURCE.into(),
                        line: 2,
                        column: 1,
                    },
                }]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime
            .evaluate_modules(
                &entry,
                BTreeMap::from([(
                    entry.clone(),
                    "export function run(input){\n return input;\n}\nglobalThis.run=run;".into(),
                )]),
            )
            .unwrap();
        assert_eq!(
            runtime
                .execute_string(
                    "invoke",
                    "String(Array.from({length:80},(_,i)=>run(i)).length)",
                    32
                )
                .unwrap(),
            "80"
        );
        std::thread::sleep(Duration::from_millis(1050));
        runtime.poll_debugger();
        assert!(!runtime.debugger_stopped());
    });
    engine.join().unwrap();
    let outputs = events
        .try_iter()
        .filter_map(|event| {
            if let DebugEventKind::Logpoint { omitted, .. } = event.event {
                Some(omitted)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(outputs.iter().filter(|omitted| **omitted == 0).count(), 8);
    assert_eq!(outputs.iter().sum::<u64>(), 72);
}

#[test]
#[allow(clippy::too_many_lines)] // One real worker proves scheduling, location stepping, and skip restoration together.
fn pause_next_reaches_user_code_and_continue_to_does_not_persist_a_breakpoint() {
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let (start, starts) = mpsc::sync_channel(1);
    let (finish_tx, complete_rx) = mpsc::sync_channel(1);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime.evaluate_modules(&entry, BTreeMap::from([(entry.clone(), "export function run(){\n let value=4;\n value++;\n return String(value);\n}\nglobalThis.run=run;globalThis.later=()=>{debugger;return 'after'};".into())])).unwrap();
        for invocation in 1..=3 {
            while starts.try_recv().is_err() {
                runtime.poll_debugger();
                std::thread::sleep(Duration::from_millis(1));
            }
            runtime.begin_debug_operation(invocation);
            assert_eq!(
                runtime
                    .execute_string(
                        "transmog:invoke",
                        if invocation <= 2 { "run()" } else { "later()" },
                        32
                    )
                    .unwrap(),
                if invocation <= 2 { "5" } else { "after" }
            );
            finish_tx.send(()).unwrap();
        }
    });
    event(&events, |event| {
        matches!(event.event, DebugEventKind::Attached)
    });
    command(&commands, 1, DebugAction::PauseNext { enabled: true });
    assert!(matches!(
        reply(&events, 1).event,
        DebugEventKind::Control {
            pause_next: true,
            ..
        }
    ));
    command(
        &commands,
        2,
        DebugAction::Evaluate {
            expression: "1+1".into(),
            pause: 0,
            frame: None,
        },
    );
    event(&events, |event| {
        event.request == 2 && matches!(event.event, DebugEventKind::Evaluated { .. })
    });
    start.send(()).unwrap();
    let paused = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    });
    assert_eq!(paused.invocation, 1);
    let DebugEventKind::Paused {
        pause,
        frames,
        reason,
        ..
    } = paused.event
    else {
        unreachable!()
    };
    assert_eq!(reason, "next callback");
    assert_eq!(frames[0].location.as_ref().unwrap().source, SOURCE);
    command(
        &commands,
        3,
        DebugAction::ContinueTo {
            pause,
            location: SourceLocation {
                source: SOURCE.into(),
                line: 4,
                column: 1,
            },
        },
    );
    assert!(matches!(
        reply(&events, 3).event,
        DebugEventKind::Acknowledged
    ));
    let DebugEventKind::Paused { pause, frames, .. } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    assert_eq!(frames[0].location.as_ref().unwrap().line, 4);
    command(
        &commands,
        4,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Continue,
        },
    );
    reply(&events, 4);
    complete_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    start.send(()).unwrap();
    complete_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Continue to here must not leave a breakpoint for later callbacks");
    command(&commands, 5, DebugAction::SkipPauses { skip: true });
    assert!(matches!(
        reply(&events, 5).event,
        DebugEventKind::Control {
            skip_pauses: true,
            ..
        }
    ));
    command(
        &commands,
        6,
        DebugAction::Evaluate {
            expression: "debugger; 1".into(),
            pause: 0,
            frame: None,
        },
    );
    assert!(matches!(
        event(&events, |event| event.request == 6
            && matches!(event.event, DebugEventKind::Evaluated { .. }))
        .event,
        DebugEventKind::Evaluated { thrown: false, .. }
    ));
    start.send(()).unwrap();
    complete_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    engine.join().unwrap();
    assert!(
        !events
            .try_iter()
            .any(|event| matches!(event.event, DebugEventKind::Paused { .. }))
    );
}
