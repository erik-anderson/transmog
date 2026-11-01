use super::*;
use std::sync::mpsc::{self, SyncSender};
use transmog_script_diagnostics::debugger::{DebugProperty, DebugValue, SourceBreakpoint};

const SESSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SOURCE: &str = "file:///transmog-script/main.js";
const FIXTURE: &str = "globalThis.touches=0;\nexport function inner(input) {\n  const local=input+4;\n  const object={get secret(){touches++;throw new Error('getter ran');}};\n  const proxy=new Proxy({}, {ownKeys(){touches++;throw 0;},get(){touches++;throw 0;}});\n  const result=local*2;\n  return result;\n}\nexport function outer(value){return inner(value);}\nglobalThis.entry=outer;\n//# sourceURL=file:///forged-source.js\n";
fn options(breakpoints: Vec<SourceBreakpoint>) -> DebugOptions {
    DebugOptions {
        session: SESSION.into(),
        exceptions: ExceptionPolicy::None,
        breakpoints,
    }
}
fn command(sender: &SyncSender<DebugCommand>, request: u64, action: DebugAction) {
    sender
        .send(DebugCommand {
            session: SESSION.into(),
            request,
            action,
        })
        .unwrap();
}
fn event(receiver: &Receiver<DebugEvent>, predicate: impl Fn(&DebugEvent) -> bool) -> DebugEvent {
    let mut observed = Vec::new();
    loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| {
                panic!("{error:?}; observed native fixture events: {observed:?}")
            });
        if predicate(&event) {
            return event;
        }
        observed.push(event.event);
    }
}
fn reply(receiver: &Receiver<DebugEvent>, request: u64) -> DebugEvent {
    event(receiver, |event| {
        event.request == request && !matches!(event.event, DebugEventKind::Sources { .. })
    })
}

#[test]
fn exception_policies_distinguish_javascript_catch_from_uncaught_throws() {
    for (policy, expected) in [
        (ExceptionPolicy::None, 0),
        (ExceptionPolicy::Uncaught, 1),
        (ExceptionPolicy::All, 2),
    ] {
        let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
        let (output, events) = mpsc::sync_channel(32);
        let engine = std::thread::spawn(move || {
            let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
            let mut config = options(vec![]);
            config.exceptions = policy;
            runtime
                .enable_debugger(
                    config,
                    DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
                )
                .unwrap();
            let entry = url::Url::parse(SOURCE).unwrap();
            runtime.evaluate_modules(&entry,BTreeMap::from([(entry.clone(),"export function entry(){try {throw new Error('caught')} catch {} throw new Error('uncaught')}globalThis.entry=entry;".into())])).unwrap();
            runtime.begin_debug_operation(1);
            assert!(
                runtime
                    .execute_script("transmog:invoke", "entry()")
                    .is_err()
            );
            runtime.poll_tasks().unwrap();
        });
        let mut pauses = 0;
        while !engine.is_finished() {
            if let Ok(DebugEvent {
                event: DebugEventKind::Paused { pause, .. },
                ..
            }) = events.recv_timeout(Duration::from_millis(20))
            {
                pauses += 1;
                command(
                    &commands,
                    pauses,
                    DebugAction::Resume {
                        pause,
                        action: ResumeAction::Continue,
                    },
                );
            }
        }
        engine.join().unwrap();
        assert_eq!(pauses, expected, "{policy:?}");
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One end-to-end pause/step/handle-lifetime transaction.
fn registered_script_ids_steps_scopes_and_inert_properties_share_one_pause_epoch() {
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let (done, result) = mpsc::sync_channel(1);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![SourceBreakpoint {
                    id: 1,
                    location: SourceLocation {
                        source: SOURCE.into(),
                        line: 6,
                        column: 1,
                    },
                }]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime
            .evaluate_modules(&entry, BTreeMap::from([(entry.clone(), FIXTURE.into())]))
            .unwrap();
        runtime.begin_debug_operation(7);
        let value = runtime.execute_string("transmog:invoke", "`${entry(3)}:${touches}`", 100);
        let _ = done.send(value.map_err(|error| error.to_string()));
    });
    let paused = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    });
    assert_eq!(paused.invocation, 7);
    let DebugEventKind::Paused { pause, frames, .. } = paused.event else {
        unreachable!()
    };
    assert_eq!(frames[0].function, "inner");
    assert_eq!(frames[0].location.as_ref().unwrap().source, SOURCE);
    assert_eq!(frames[0].location.as_ref().unwrap().line, 6);
    assert!(frames.iter().any(|frame| frame.function == "outer"));
    command(
        &commands,
        1,
        DebugAction::Scopes {
            pause: pause + 1,
            frame: 0,
        },
    );
    assert!(matches!(
        reply(&events, 1).event,
        DebugEventKind::Rejected { .. }
    ));
    command(&commands, 2, DebugAction::Scopes { pause, frame: 0 });
    let DebugEventKind::Scopes { scopes, .. } = reply(&events, 2).event else {
        panic!("scope read failed")
    };
    let handle = scopes
        .iter()
        .find(|scope| scope.kind == "local")
        .unwrap()
        .handle
        .unwrap();
    command(
        &commands,
        3,
        DebugAction::Properties {
            pause,
            handle,
            offset: 0,
        },
    );
    let DebugEventKind::Properties { properties, .. } = reply(&events, 3).event else {
        panic!("locals unavailable")
    };
    assert_eq!(
        properties
            .iter()
            .find(|property| property.name == "local")
            .unwrap()
            .value
            .display,
        "7"
    );
    let object = properties
        .iter()
        .find(|property| property.name == "object")
        .unwrap()
        .value
        .handle
        .unwrap();
    assert!(
        properties
            .iter()
            .find(|property| property.name == "proxy")
            .unwrap()
            .value
            .handle
            .is_none()
    );
    command(
        &commands,
        4,
        DebugAction::Properties {
            pause,
            handle: object,
            offset: 0,
        },
    );
    let DebugEventKind::Properties { properties, .. } = reply(&events, 4).event else {
        panic!("object unavailable")
    };
    assert!(
        properties
            .iter()
            .any(|property| property.name == "secret" && property.value.kind == "unavailable")
    );
    command(
        &commands,
        5,
        DebugAction::Release {
            pause,
            handles: vec![object],
        },
    );
    assert!(matches!(
        reply(&events, 5).event,
        DebugEventKind::Acknowledged
    ));
    command(
        &commands,
        6,
        DebugAction::Properties {
            pause,
            handle: object,
            offset: 0,
        },
    );
    assert!(matches!(
        reply(&events, 6).event,
        DebugEventKind::Rejected { .. }
    ));
    command(
        &commands,
        7,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Over,
        },
    );
    assert!(matches!(
        reply(&events, 7).event,
        DebugEventKind::Acknowledged
    ));
    let DebugEventKind::Paused {
        pause: next,
        frames,
        ..
    } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    assert!(next > pause);
    assert_eq!(frames[0].location.as_ref().unwrap().line, 7);
    command(
        &commands,
        8,
        DebugAction::Properties {
            pause,
            handle,
            offset: 0,
        },
    );
    assert!(matches!(
        reply(&events, 8).event,
        DebugEventKind::Rejected { .. }
    ));
    command(
        &commands,
        9,
        DebugAction::Resume {
            pause: next,
            action: ResumeAction::Continue,
        },
    );
    assert!(matches!(
        reply(&events, 9).event,
        DebugEventKind::Acknowledged
    ));
    assert_eq!(
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        "14:0"
    );
    engine.join().unwrap();
}

#[test]
fn idle_detach_preserves_globals_and_absolute_expiry_cannot_be_extended() {
    let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
    runtime
        .execute_script("transmog:bootstrap", "globalThis.count=1;")
        .unwrap();
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    runtime
        .enable_debugger(
            options(vec![]),
            DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
        )
        .unwrap();
    command(&commands, 1, DebugAction::Detach);
    drop(commands);
    runtime.poll_debugger();
    assert!(runtime.debugger.is_none());
    assert!(matches!(reply(&events, 1).event, DebugEventKind::Detached));
    assert_eq!(
        runtime
            .execute_string("transmog:invoke", "String(++count)", 32)
            .unwrap(),
        "2"
    );
    let (_commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    runtime
        .enable_debugger(
            options(vec![]),
            DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
        )
        .unwrap();
    runtime.debugger.as_ref().unwrap().client.created.set(
        Instant::now()
            .checked_sub(Duration::from_millis(SESSION_LEASE_MS))
            .unwrap(),
    );
    runtime.poll_debugger();
    assert!(runtime.debugger_stopped());
    assert!(matches!(
        event(&events, |event| matches!(
            event.event,
            DebugEventKind::Stopped { .. }
        ))
        .event,
        DebugEventKind::Stopped { .. }
    ));
}

#[test]
fn exception_pause_never_invokes_error_getters_or_formats_authored_objects() {
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let (done, result) = mpsc::sync_channel(1);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        let mut config = options(vec![]);
        config.exceptions = ExceptionPolicy::All;
        runtime
            .enable_debugger(
                config,
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime.evaluate_modules(&entry,BTreeMap::from([(entry.clone(),"globalThis.touches=0; export function fail(){const error=new Error();Object.defineProperty(error,'message',{get(){touches++;return 'unsafe';}});Object.defineProperty(error,'stack',{get(){touches++;return 'unsafe';}});throw error;} globalThis.entry=fail;".into())])).unwrap();
        runtime.begin_debug_operation(9);
        let failed = runtime
            .execute_script("transmog:invoke", "entry()")
            .is_err();
        let calls = runtime.execute_string("transmog:check", "String(touches)", 32);
        let _ = done.send((failed, calls.map_err(|error| error.to_string())));
    });
    let DebugEventKind::Paused {
        pause, exception, ..
    } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    assert!(exception.is_some());
    assert!(exception.unwrap().exception.unavailable);
    command(
        &commands,
        1,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Continue,
        },
    );
    let _ = reply(&events, 1);
    let (failed, calls) = result.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(failed);
    assert_eq!(calls.unwrap(), "0");
    engine.join().unwrap();
}

#[test]
#[allow(clippy::too_many_lines)] // Trace actual initializer, step-into and step-out order in one worker.
fn source_breakpoints_pause_initializers_before_side_effects_and_step_into_and_out() {
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let (done, result) = mpsc::sync_channel(1);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![
                    SourceBreakpoint {
                        id: 1,
                        location: SourceLocation {
                            source: SOURCE.into(),
                            line: 1,
                            column: 1,
                        },
                    },
                    SourceBreakpoint {
                        id: 2,
                        location: SourceLocation {
                            source: SOURCE.into(),
                            line: 9,
                            column: 1,
                        },
                    },
                ]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime
            .evaluate_modules(&entry, BTreeMap::from([(entry.clone(), FIXTURE.into())]))
            .unwrap();
        runtime.begin_debug_operation(11);
        let value = runtime.execute_string("transmog:invoke", "`${entry(3)}:${touches}`", 100);
        let _ = done.send(value.map_err(|error| error.to_string()));
    });
    let paused = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    });
    assert_eq!(paused.invocation, 0);
    let DebugEventKind::Paused { pause, frames, .. } = paused.event else {
        unreachable!()
    };
    assert_eq!(frames[0].location.as_ref().unwrap().line, 1);
    command(
        &commands,
        1,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Continue,
        },
    );
    let _ = reply(&events, 1);
    let DebugEventKind::Paused { pause, frames, .. } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    assert_eq!(frames[0].function, "outer");
    command(
        &commands,
        2,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Into,
        },
    );
    let _ = reply(&events, 2);
    let DebugEventKind::Paused { pause, frames, .. } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    assert_eq!(frames[0].function, "inner");
    command(
        &commands,
        3,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Out,
        },
    );
    let _ = reply(&events, 3);
    let DebugEventKind::Paused { pause, frames, .. } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    assert_ne!(frames[0].function, "inner");
    command(
        &commands,
        4,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Continue,
        },
    );
    let _ = reply(&events, 4);
    assert_eq!(
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        "14:0"
    );
    engine.join().unwrap();
}

#[test]
fn controller_loss_stops_a_paused_engine_and_default_runtime_ignores_debugger_statements() {
    let mut ordinary = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
    assert_eq!(
        ordinary
            .execute_string("transmog:invoke", "debugger; 'ordinary'", 32)
            .unwrap(),
        "ordinary"
    );
    drop(ordinary);
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let (done, result) = mpsc::sync_channel(1);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let failed = runtime
            .execute_script("transmog:invoke", "debugger; while(true){}")
            .is_err();
        let _ = done.send(failed);
    });
    let _ = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    });
    drop(commands);
    let _ = event(&events, |event| {
        matches!(event.event, DebugEventKind::Stopped { .. })
    });
    assert!(result.recv_timeout(Duration::from_secs(5)).unwrap());
    engine.join().unwrap();
}

#[test]
fn wire_contract_cannot_express_evaluation_mutation_or_conditional_breakpoints() {
    for action in [
        json!({"kind":"evaluate","expression":"secret"}),
        json!({"kind":"resume","pause":1,"action":"continue","expression":"touches++"}),
        json!({"kind":"properties","pause":1,"handle":1,"offset":0,"invokeGetters":true}),
        json!({"kind":"breakpoints","values":[{"id":1,"location":{"source":SOURCE,"line":1,"column":1},"condition":"touches++"}]}),
    ] {
        assert!(
            serde_json::from_value::<DebugCommand>(
                json!({"session":SESSION,"request":1,"action":action})
            )
            .is_err()
        );
    }
    let event = DebugEvent {
        session: SESSION.into(),
        invocation: 1,
        request: 1,
        event: DebugEventKind::Properties {
            pause: 1,
            handle: 1,
            offset: 0,
            properties: vec![DebugProperty {
                name: "fixture".into(),
                value: DebugValue {
                    kind: "string".into(),
                    display: "private-local-canary".into(),
                    handle: None,
                    truncated: false,
                },
            }],
            next: None,
        },
    };
    assert!(!format!("{event:?}").contains("private-local-canary"));
}

#[test]
#[allow(clippy::too_many_lines)] // One real pause proves scope order and value/handle budgets together.
fn lexical_scopes_preserve_shadowing_closures_and_bounded_property_pages() {
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(32);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime.evaluate_modules(&entry,BTreeMap::from([(entry.clone(),"function factory(){let outer=41;let shadow=1;globalThis.readOuter=()=>shadow;return function inner(){let shadow=7;const many=Object.fromEntries(Array.from({length:150},(_,i)=>['key'+i,{value:i}]));{let shadow=11;debugger;return shadow+outer;}}}globalThis.entry=factory();".into())])).unwrap();
        assert_eq!(
            runtime
                .execute_string("transmog:invoke", "String(entry())", 100)
                .unwrap(),
            "52"
        );
    });
    let DebugEventKind::Paused { pause, .. } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    command(&commands, 1, DebugAction::Scopes { pause, frame: 0 });
    let DebugEventKind::Scopes { scopes, .. } = reply(&events, 1).event else {
        unreachable!()
    };
    let mut request = 2;
    let mut shadows = Vec::new();
    let mut closure = false;
    let mut many = None;
    for scope in scopes
        .iter()
        .filter(|scope| matches!(scope.kind.as_str(), "block" | "local" | "closure"))
    {
        let Some(handle) = scope.handle else { continue };
        command(
            &commands,
            request,
            DebugAction::Properties {
                pause,
                handle,
                offset: 0,
            },
        );
        let DebugEventKind::Properties { properties, .. } = reply(&events, request).event else {
            unreachable!()
        };
        request += 1;
        for property in properties {
            if property.name == "shadow" {
                shadows.push(property.value.display.clone());
            }
            if property.name == "outer" && property.value.display == "41" {
                closure = true;
            }
            if property.name == "many" {
                many = property.value.handle;
            }
        }
    }
    assert!(shadows.starts_with(&["11".into(), "7".into()]));
    assert!(closure);
    let handle = many.unwrap();
    command(
        &commands,
        request,
        DebugAction::Properties {
            pause,
            handle,
            offset: 0,
        },
    );
    let DebugEventKind::Properties {
        properties, next, ..
    } = reply(&events, request).event
    else {
        unreachable!()
    };
    request += 1;
    assert_eq!(properties.len(), 100);
    assert_eq!(next, Some(100));
    assert!(
        properties
            .iter()
            .any(|property| property.value.handle.is_none()
                && property.value.display.contains("handle limit"))
    );
    command(
        &commands,
        request,
        DebugAction::Properties {
            pause,
            handle,
            offset: 100,
        },
    );
    let DebugEventKind::Properties {
        properties, next, ..
    } = reply(&events, request).event
    else {
        unreachable!()
    };
    request += 1;
    assert_eq!(properties.len(), 50);
    assert_eq!(next, None);
    command(
        &commands,
        request,
        DebugAction::Resume {
            pause,
            action: ResumeAction::Continue,
        },
    );
    let _ = reply(&events, request);
    engine.join().unwrap();
}
