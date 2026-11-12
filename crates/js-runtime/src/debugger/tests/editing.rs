use super::*;

#[test]
#[allow(clippy::too_many_lines)] // One real pause verifies exact shadowed bindings and inert property edits.
fn value_edits_target_exact_bindings_and_writable_data_without_running_hooks() {
    let (commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
    let (output, events) = mpsc::sync_channel(64);
    let engine = std::thread::spawn(move || {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_debugger(
                options(vec![]),
                DebugTransport::new(input, move |event| output.try_send(event).is_ok()),
            )
            .unwrap();
        let entry = url::Url::parse(SOURCE).unwrap();
        runtime
            .evaluate_modules(
                &entry,
                BTreeMap::from([(
                    entry.clone(),
                    r#"
            globalThis.touches=0;
            export function run(){
                let value=10;
                function inner(){
                    let value=20;
                    let object={"odd.key":"before", get secret(){touches++;return 1}};
                    Object.defineProperty(object,"locked",{value:1,writable:false});
                    debugger;
                    return value+'|'+object['odd.key']+'|'+object.locked+'|'+touches;
                }
                return inner()+'|'+value;
            }
            globalThis.run=run;
        "#
                    .into(),
                )]),
            )
            .unwrap();
        assert_eq!(
            runtime.execute_string("invoke", "run()", 128).unwrap(),
            "40|after|1|0|11"
        );
    });
    let DebugEventKind::Paused { pause, .. } = event(&events, |event| {
        matches!(event.event, DebugEventKind::Paused { .. })
    })
    .event
    else {
        unreachable!()
    };
    let mut serial = 0;
    let mut send = |action| {
        serial += 1;
        command(&commands, serial, action);
        event(&events, |event| {
            event.request == serial
                && !matches!(
                    event.event,
                    DebugEventKind::Sources { .. }
                        | DebugEventKind::Evaluating
                        | DebugEventKind::Output { .. }
                )
        })
        .event
    };
    let mut bindings = Vec::new();
    let mut object = 0;
    for frame in [0, 1] {
        let DebugEventKind::Scopes { scopes, .. } = send(DebugAction::Scopes { pause, frame })
        else {
            unreachable!()
        };
        let handle = scopes
            .iter()
            .find(|scope| scope.kind == "local")
            .unwrap()
            .handle
            .unwrap();
        let DebugEventKind::Properties { properties, .. } = send(DebugAction::Properties {
            pause,
            handle,
            offset: 0,
        }) else {
            unreachable!()
        };
        for (index, property) in properties.iter().enumerate() {
            if property.name == "value" {
                bindings.push((
                    frame,
                    handle,
                    u32::try_from(index).unwrap(),
                    property.value.display.clone(),
                ));
            }
            if property.name == "object" {
                object = property.value.handle.unwrap();
            }
        }
    }
    assert_eq!(bindings.len(), 2);
    for (frame, handle, index, previous) in &bindings {
        let replacement = if previous == "20" { "40" } else { "11" };
        assert!(matches!(
            send(DebugAction::SetValue {
                pause,
                frame: *frame,
                handle: *handle,
                index: *index,
                expression: replacement.into()
            }),
            DebugEventKind::Acknowledged
        ));
        let DebugEventKind::Scopes { scopes, .. } = send(DebugAction::Scopes {
            pause,
            frame: *frame,
        }) else {
            unreachable!()
        };
        let refreshed = scopes.iter().find(|scope| scope.kind == "local").unwrap();
        let DebugEventKind::Properties { properties, .. } = send(DebugAction::Properties {
            pause,
            handle: refreshed.handle.unwrap(),
            offset: 0,
        }) else {
            unreachable!()
        };
        assert_eq!(
            properties
                .iter()
                .find(|property| property.name == "value")
                .unwrap()
                .value
                .display,
            replacement,
            "reacquired scopes must show the edited binding"
        );
    }
    assert!(matches!(
        send(DebugAction::Evaluate {
            pause,
            frame: Some(0),
            expression: "value=41".into()
        }),
        DebugEventKind::Evaluated { thrown: false, .. }
    ));
    send(DebugAction::SkipPauses { skip: true });
    let DebugEventKind::Scopes { scopes, .. } = send(DebugAction::Scopes { pause, frame: 0 })
    else {
        unreachable!()
    };
    let handle = scopes
        .iter()
        .find(|scope| scope.kind == "local")
        .unwrap()
        .handle
        .unwrap();
    let DebugEventKind::PropertyPreview { properties, .. } = send(DebugAction::PreviewProperties {
        pause,
        handle,
        offset: 0,
        limit: 100,
        refresh: true,
    }) else {
        unreachable!()
    };
    assert_eq!(
        properties
            .iter()
            .find(|property| property.name == "value")
            .unwrap()
            .value
            .display,
        "41",
        "reacquired scope previews must also reflect Console assignments"
    );
    send(DebugAction::SkipPauses { skip: false });
    let (frame, handle, index, _) = bindings[0];
    assert!(matches!(
        send(DebugAction::SetValue {
            pause,
            frame,
            handle,
            index,
            expression: "40".into()
        }),
        DebugEventKind::Acknowledged
    ));
    let DebugEventKind::Properties { properties, .. } = send(DebugAction::Properties {
        pause,
        handle: object,
        offset: 0,
    }) else {
        unreachable!()
    };
    let index = u32::try_from(
        properties
            .iter()
            .position(|property| property.name == "odd.key")
            .unwrap(),
    )
    .unwrap();
    for (expression, kind, display) in [
        ("Infinity", "number", "Infinity"),
        ("-Infinity", "number", "-Infinity"),
        ("-0", "number", "-0"),
        ("42n", "bigint", "42"),
        ("undefined", "undefined", "undefined"),
        ("({answer:2})", "object", "[Object]"),
    ] {
        assert!(matches!(
            send(DebugAction::SetValue {
                pause,
                frame: 0,
                handle: object,
                index,
                expression: expression.into()
            }),
            DebugEventKind::Acknowledged
        ));
        let DebugEventKind::Properties { properties, .. } = send(DebugAction::Properties {
            pause,
            handle: object,
            offset: 0,
        }) else {
            unreachable!()
        };
        let value = &properties
            .iter()
            .find(|property| property.name == "odd.key")
            .unwrap()
            .value;
        assert_eq!(
            (value.kind.as_str(), value.display.as_str()),
            (kind, display)
        );
    }
    for (index, property) in properties.iter().enumerate() {
        let expression = if property.name == "odd.key" {
            "'after'"
        } else {
            "touches++"
        };
        let result = send(DebugAction::SetValue {
            pause,
            frame: 0,
            handle: object,
            index: u32::try_from(index).unwrap(),
            expression: expression.into(),
        });
        assert_eq!(property.writable, property.name == "odd.key");
        assert_eq!(
            matches!(result, DebugEventKind::Acknowledged),
            property.writable
        );
    }
    let (frame, handle, index, _) = bindings[0];
    assert!(matches!(
        send(DebugAction::SetValue {
            pause,
            frame,
            handle,
            index,
            expression: "touches++".into()
        }),
        DebugEventKind::Rejected { .. }
    ));
    assert!(matches!(
        send(DebugAction::SetValue {
            pause: pause + 1,
            frame,
            handle,
            index,
            expression: "99".into()
        }),
        DebugEventKind::Rejected { .. }
    ));
    send(DebugAction::Resume {
        pause,
        action: ResumeAction::Continue,
    });
    engine.join().unwrap();
}
