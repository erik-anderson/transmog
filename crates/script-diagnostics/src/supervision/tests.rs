use super::*;
use crate::debugger::{ExceptionPolicy, ResumeAction};
use std::sync::atomic::{AtomicBool, Ordering};
fn session() -> Arc<Session> {
    Session::new(DebugOptions {
        skip_pauses: false,
        pause_on_start: false,
        persistent: false,
        session: "a".repeat(32),
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    })
    .unwrap()
}
#[test]
fn property_reads_do_not_start_or_resume_another_evaluation() {
    let session = session();
    session.bind(|_| true, || {});
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    let evaluation = session
        .command(DebugAction::Evaluate {
            expression: "slow()".into(),
            pause: 0,
            frame: None,
        })
        .unwrap();
    let mut started = event(&session, DebugEventKind::Evaluating);
    started.request = evaluation;
    session.accept(started.clone()).unwrap();
    let property = session
        .command(DebugAction::Property {
            pause: 0,
            handle: 1,
            index: 0,
        })
        .unwrap();
    started.request = property;
    assert!(session.accept(started).is_err());
    let mut reply = event(
        &session,
        DebugEventKind::Evaluated {
            pause: 0,
            value: crate::debugger::DebugValue {
                kind: "object".into(),
                display: "[Object]".into(),
                handle: Some(2),
                truncated: false,
            },
            thrown: false,
        },
    );
    reply.request = property;
    session.accept(reply.clone()).unwrap();
    assert!(
        session.clock.0.lock().unwrap().1.is_some(),
        "A property read must preserve an active evaluation's blocking clock"
    );
    reply.request = evaluation;
    session.accept(reply).unwrap();
    assert!(session.clock.0.lock().unwrap().1.is_none());
}

#[test]
fn property_previews_enforce_the_requested_range_and_do_not_accept_value_handles() {
    let session = session();
    session.bind(|_| true, || {});
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    for limit in [0, 101] {
        assert!(
            session
                .command(DebugAction::PreviewProperties {
                    pause: 0,
                    handle: 1,
                    offset: 0,
                    limit,
                    refresh: false
                })
                .is_err()
        );
    }
    let request = session
        .command(DebugAction::PreviewProperties {
            pause: 0,
            handle: 1,
            offset: 0,
            limit: 1,
            refresh: true,
        })
        .unwrap();
    let property = crate::debugger::DebugProperty {
        name: "item".into(),
        value: crate::debugger::DebugValue {
            kind: "object".into(),
            display: "[Object]".into(),
            handle: None,
            truncated: false,
        },
    };
    let mut reply = event(
        &session,
        DebugEventKind::PropertyPreview {
            pause: 0,
            handle: 1,
            offset: 0,
            total: 3,
            properties: vec![property],
            next: Some(1),
        },
    );
    reply.request = request;
    if let DebugEventKind::PropertyPreview { properties, .. } = &mut reply.event {
        properties[0].value.handle = Some(2);
    }
    assert!(session.accept(reply.clone()).is_err());
    if let DebugEventKind::PropertyPreview { properties, .. } = &mut reply.event {
        properties[0].value.handle = None;
        properties.push(properties[0].clone());
    }
    assert!(session.accept(reply.clone()).is_err());
    if let DebugEventKind::PropertyPreview { properties, .. } = &mut reply.event {
        properties.pop();
    }
    if let DebugEventKind::PropertyPreview { next, .. } = &mut reply.event {
        *next = None;
    }
    assert!(session.accept(reply.clone()).is_err());
    if let DebugEventKind::PropertyPreview { next, .. } = &mut reply.event {
        *next = Some(1);
    }
    session.accept(reply).unwrap();
}

#[test]
fn inspection_replies_are_correlated_bounded_and_valid_in_the_exact_scope() {
    let session = session();
    session.bind(|_| true, || {});
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    let request = session
        .command(DebugAction::Complete {
            expression: "Math.a".into(),
            pause: 0,
            frame: None,
        })
        .unwrap();
    let mut reply = event(
        &session,
        DebugEventKind::Completions {
            pause: 0,
            matches: vec!["abs".into()],
            omitted: false,
        },
    );
    assert!(session.accept(reply.clone()).is_err());
    reply.request = request;
    if let DebugEventKind::Completions { matches, .. } = &mut reply.event {
        matches[0] = "x".repeat(257);
    }
    assert!(session.accept(reply.clone()).is_err());
    if let DebugEventKind::Completions { matches, .. } = &mut reply.event {
        matches[0] = "abs".into();
    }
    session.accept(reply.clone()).unwrap();
    assert!(session.accept(reply).is_err());
    let request = session
        .command(DebugAction::Properties {
            pause: 0,
            handle: 1,
            offset: 0,
        })
        .unwrap();
    let mut reply = event(
        &session,
        DebugEventKind::Properties {
            pause: 0,
            handle: 1,
            offset: 0,
            properties: vec![],
            next: None,
        },
    );
    reply.request = request;
    session.accept(reply).unwrap();
    session.begin(0);
    session.accept(event(&session, paused())).unwrap();
    assert!(
        session
            .command(DebugAction::Inspect {
                expression: "value".into(),
                pause: 0,
                frame: None
            })
            .is_err()
    );
    assert!(
        session
            .command(DebugAction::Properties {
                pause: 0,
                handle: 1,
                offset: 0
            })
            .is_err()
    );
    session.stop("Test ended");
}

#[test]
fn queued_inspection_waits_for_the_running_command_and_receives_a_fresh_progress_budget() {
    let session = session();
    session.bind(|_| true, || {});
    session.begin(0);
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session.accept(event(&session, paused())).unwrap();
    let evaluate = session
        .command(DebugAction::Evaluate {
            expression: "value".into(),
            pause: 1,
            frame: Some(0),
        })
        .unwrap();
    let mut started = event(&session, DebugEventKind::Evaluating);
    started.request = evaluate;
    session.accept(started).unwrap();
    let inspect = session
        .command(DebugAction::Scopes { pause: 1, frame: 0 })
        .unwrap();
    {
        let mut state = session.state.lock().unwrap();
        for pending in state.pending.values_mut() {
            pending.since = Instant::now()
                .checked_sub(Duration::from_millis(700))
                .unwrap();
        }
    }
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        !session.is_closed(),
        "A queued read is not a stalled inspection"
    );
    let mut evaluated = event(
        &session,
        DebugEventKind::Evaluated {
            pause: 1,
            value: crate::debugger::DebugValue {
                kind: "number".into(),
                display: "1".into(),
                handle: None,
                truncated: false,
            },
            thrown: false,
        },
    );
    evaluated.request = evaluate;
    session.accept(evaluated).unwrap();
    assert!(
        session.state.lock().unwrap().pending[&inspect]
            .since
            .elapsed()
            < Duration::from_millis(100)
    );
    session.stop("Test ended");
}

#[test]
fn automatic_logpoint_inspection_suspends_budgets_and_has_authenticated_start_and_end() {
    let session = session();
    session.bind(|_| true, || {});
    session.begin(0);
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session
        .accept(event(
            &session,
            DebugEventKind::Sources {
                sources: vec![crate::debugger::DebugSource {
                    source: "file:///main.js".into(),
                    bytes: None,
                }],
                breakpoints: vec![],
            },
        ))
        .unwrap();
    let output = DebugEventKind::Logpoint {
        location: Some(crate::SourceLocation {
            source: "file:///main.js".into(),
            line: 1,
            column: 1,
        }),
        original: None,
        text: "value".into(),
        thrown: false,
        omitted: 0,
        time_unix_ms: 0,
    };
    assert!(session.accept(event(&session, output.clone())).is_err());
    session
        .accept(event(&session, DebugEventKind::LogpointStarted))
        .unwrap();
    assert!(
        session
            .accept(event(&session, DebugEventKind::LogpointStarted))
            .is_err()
    );
    std::thread::sleep(Duration::from_millis(30));
    session.accept(event(&session, output)).unwrap();
    let elapsed = session.clock.elapsed();
    assert!(elapsed >= Duration::from_millis(25));
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(session.clock.elapsed(), elapsed);
    session.stop("Test ended");
}
#[test]
fn workspace_detaches_after_a_queued_attachment_and_keeps_pause_snapshot() {
    let session = Session::new(DebugOptions {
        skip_pauses: false,
        pause_on_start: false,
        persistent: true,
        session: "b".repeat(32),
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    })
    .unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    session.bind(
        move |command| sender.send(command).is_ok(),
        || panic!("Healthy detach must not stop the worker"),
    );
    session.request_detach().unwrap();
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    let command = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(command.action, DebugAction::Detach));
    let mut detached = event(&session, DebugEventKind::Detached);
    detached.request = command.request;
    session.accept(detached).unwrap();
    assert!(session.detached());
}
#[test]
fn workspace_pause_survives_legacy_lease_and_history_eviction() {
    let session = Session::new(DebugOptions {
        skip_pauses: false,
        pause_on_start: false,
        persistent: true,
        session: "c".repeat(32),
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    })
    .unwrap();
    session.bind(|_| true, || {});
    session.begin(0);
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session.accept(event(&session, paused())).unwrap();
    session.state.lock().unwrap().paused_at =
        Some(Instant::now().checked_sub(Duration::from_secs(90)).unwrap());
    std::thread::sleep(Duration::from_millis(30));
    assert!(!session.is_closed());
    for _ in 0..80 {
        let request = session
            .command(DebugAction::Exceptions {
                policy: ExceptionPolicy::None,
            })
            .unwrap();
        let mut reply = event(&session, DebugEventKind::Acknowledged);
        reply.request = request;
        session.accept(reply).unwrap();
    }
    assert!(matches!(
        session.page(0).paused.unwrap().event,
        DebugEventKind::Paused { pause: 1, .. }
    ));
    session.stop("Test ended");
}
#[tokio::test]
async fn dependent_budgets_exclude_separate_worker_pauses_and_keep_the_active_limit() {
    let first = Arc::new(PauseClock::default());
    let second = Arc::new(PauseClock::default());
    let clocks = [Arc::clone(&first), Arc::clone(&first), Arc::clone(&second)];
    wait_budgets(Duration::from_millis(40), &clocks, async move {
        first.pause();
        tokio::time::sleep(Duration::from_millis(70)).await;
        first.resume();
        second.pause();
        tokio::time::sleep(Duration::from_millis(70)).await;
        second.resume();
    })
    .await
    .unwrap();
    assert!(
        wait_budgets(
            Duration::from_millis(20),
            &clocks,
            std::future::pending::<()>()
        )
        .await
        .is_err()
    );
}
fn event(session: &Session, event: DebugEventKind) -> DebugEvent {
    DebugEvent {
        value_generation: None,
        session: session.id().into(),
        invocation: 0,
        request: 0,
        event,
    }
}
fn paused() -> DebugEventKind {
    DebugEventKind::Paused {
        pause: 1,
        reason: "breakpoint".into(),
        frames: vec![],
        exception: None,
        truncated: false,
    }
}
#[test]
fn current_operation_metadata_does_not_depend_on_console_output_or_leak_between_callbacks() {
    let session = session();
    let capture = crate::LogCapture::new(crate::LogContext {
        artifact: Some("script:exact".into()),
        worker: "worker".into(),
        revision: "revision".into(),
        callback: "initialize".into(),
        invocation: 0,
        exchange_id: None,
    });
    session.track_logs(Arc::clone(&capture));
    assert!(capture.complete_or_silent());
    assert!(session.page(0).operation.is_none());
    assert!(session.begin(0));
    assert_eq!(session.page(0).operation.unwrap().callback, "initialize");
    assert_eq!(session.page(0).logs, Vec::new());
    session.completed();
    assert!(session.page(0).operation.is_none());
    assert!(session.begin(1));
    assert!(session.page(0).operation.is_none());
    session.track_logs(crate::LogCapture::new(crate::LogContext {
        callback: "onResponseBody".into(),
        invocation: 1,
        exchange_id: Some("request-one".into()),
        ..capture.context()
    }));
    let page = session.page(0);
    assert_eq!(
        page.operation.unwrap().exchange_id.as_deref(),
        Some("request-one")
    );
    assert!(page.logs.iter().all(|run| run.records.is_empty()));
    session.completed();
    assert!(session.begin(2));
    assert!(session.page(0).operation.is_none());
    session.stop("Test ended");
    assert!(session.page(0).operation.is_none());
}
#[test]
fn native_session_rejects_forged_identity_duplicate_pauses_and_stale_commands() {
    let session = session();
    session.begin(0);
    let mut forged = event(&session, DebugEventKind::Attached);
    forged.value_generation = Some(1);
    assert!(session.accept(forged).is_err());
    let mut forged = event(&session, DebugEventKind::Attached);
    forged.session = "b".repeat(32);
    assert!(session.accept(forged).is_err());
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session.accept(event(&session, paused())).unwrap();
    assert_eq!(session.page(0).value_generation, 1);
    assert_eq!(session.page(0).paused.unwrap().value_generation, Some(1));
    assert!(session.accept(event(&session, paused())).is_err());
    session
        .accept(event(&session, DebugEventKind::Resumed))
        .unwrap();
    assert_eq!(session.page(0).value_generation, 1);
    assert_eq!(
        session.page(0).events.last().unwrap().value_generation,
        Some(1)
    );
    assert!(
        session
            .command(DebugAction::Resume {
                pause: 1,
                action: ResumeAction::Continue
            })
            .is_err()
    );
    assert!(
        session
            .accept(event(&session, DebugEventKind::Resumed))
            .is_err()
    );
    session.stop("Test ended");
    assert!(session.page(0).closed);
}
#[test]
fn stalled_inspection_is_killed_by_the_native_watchdog_without_ui_polling() {
    let session = session();
    let killed = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&killed);
    session.bind(|_| true, move || marker.store(true, Ordering::Release));
    session.begin(0);
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session.accept(event(&session, paused())).unwrap();
    session
        .command(DebugAction::Scopes { pause: 1, frame: 0 })
        .unwrap();
    let start = Instant::now();
    while !killed.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(killed.load(Ordering::Acquire));
    assert!(session.page(0).closed);
}
#[test]
fn detached_session_does_not_terminate_its_reusable_worker() {
    let session = session();
    let killed = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&killed);
    session.bind(|_| true, move || marker.store(true, Ordering::Release));
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session
        .accept(event(&session, DebugEventKind::Detached))
        .unwrap();
    session.stop("Window closed");
    assert!(!killed.load(Ordering::Acquire));
    assert!(session.page(0).closed);
}
#[test]
fn a_stopped_attachment_binds_termination_before_its_control_transport_exists() {
    let session = session();
    session.stop("Window was lost before attachment");
    let killed = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&killed);
    session.bind_stop(move || marker.store(true, Ordering::Release));
    assert!(killed.load(Ordering::Acquire));
}
#[test]
fn native_arming_expiry_requests_idle_detach_and_pause_expiry_kills_without_polling() {
    let session = session();
    let (commands, receiver) = std::sync::mpsc::sync_channel(1);
    let killed = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&killed);
    session.bind(
        move |command| commands.try_send(command).is_ok(),
        move || marker.store(true, Ordering::Release),
    );
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    assert!(session.arm());
    session.state.lock().unwrap().armed_at = Some(
        Instant::now()
            .checked_sub(Duration::from_millis(ARM_LEASE_MS))
            .unwrap(),
    );
    let command = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(command.action, DebugAction::Detach));
    assert!(!session.claim_exchange("late"));
    let mut detached = event(&session, DebugEventKind::Detached);
    detached.request = command.request;
    session.accept(detached).unwrap();
    assert!(!killed.load(Ordering::Acquire));
    let session = super::tests::session();
    let marker = Arc::clone(&killed);
    session.bind(|_| true, move || marker.store(true, Ordering::Release));
    session.begin(0);
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session.accept(event(&session, paused())).unwrap();
    session.state.lock().unwrap().paused_at = Some(
        Instant::now()
            .checked_sub(Duration::from_millis(PAUSE_LEASE_MS))
            .unwrap(),
    );
    let start = Instant::now();
    while !killed.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(killed.load(Ordering::Acquire));
    assert!(session.is_closed());
}
#[test]
fn inventory_survives_transient_history_and_configuration_replies_can_cross_operation_boundaries() {
    let session = session();
    session.bind(|_| true, || {});
    for index in 0..128 {
        session
            .accept(event(
                &session,
                DebugEventKind::Sources {
                    sources: vec![crate::debugger::DebugSource {
                        source: format!("file:///module-{index}.js"),
                        bytes: Some(7),
                    }],
                    breakpoints: vec![],
                },
            ))
            .unwrap();
    }
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    let request = session
        .command(DebugAction::Exceptions {
            policy: ExceptionPolicy::All,
        })
        .unwrap();
    assert!(session.begin(1));
    let mut reply = event(&session, DebugEventKind::Acknowledged);
    reply.request = request;
    session.accept(reply).unwrap();
    let page = session.page(0);
    assert!(page.omitted);
    assert_eq!(page.inventory.unwrap().sources.len(), 128);
    assert!(!session.is_closed());
    session.stop("Test ended");
}
#[test]
fn arming_claim_is_atomic_with_expiry_and_only_one_exchange_is_selected() {
    let session = session();
    assert!(session.arm());
    assert!(session.claim_exchange("first"));
    assert!(!session.claim_exchange("second"));
    assert_eq!(session.page(0).exchange_id.as_deref(), Some("first"));
    session.stop("Test ended");
    let session = super::tests::session();
    assert!(session.arm());
    session.state.lock().unwrap().armed_at = Some(
        Instant::now()
            .checked_sub(Duration::from_millis(ARM_LEASE_MS))
            .unwrap(),
    );
    assert!(!session.claim_exchange("late"));
    session.stop("Test ended");
}
#[test]
fn value_reply_must_match_the_requested_frame_and_page_and_cleanup_composes() {
    let session = session();
    session.bind(|_| true, || {});
    session.begin(0);
    session
        .accept(event(&session, DebugEventKind::Attached))
        .unwrap();
    session.accept(event(&session, paused())).unwrap();
    let request = session
        .command(DebugAction::Scopes { pause: 1, frame: 0 })
        .unwrap();
    let mut reply = event(
        &session,
        DebugEventKind::Scopes {
            pause: 1,
            frame: 1,
            scopes: vec![],
        },
    );
    reply.request = request;
    assert!(session.accept(reply).is_err());
    let cleaned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for _ in 0..2 {
        let marker = Arc::clone(&cleaned);
        session.on_closed(move || {
            marker.fetch_add(1, Ordering::Relaxed);
        });
    }
    session.stop("Test ended");
    assert_eq!(cleaned.load(Ordering::Relaxed), 2);
    let marker = Arc::clone(&cleaned);
    session.on_closed(move || {
        marker.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(cleaned.load(Ordering::Relaxed), 3);
}
#[tokio::test]
async fn active_budget_excludes_authenticated_pause_but_retains_its_execution_limit() {
    let clock = Arc::new(PauseClock::default());
    let state = Arc::clone(&clock);
    let future = async move {
        state.pause();
        tokio::time::sleep(Duration::from_millis(60)).await;
        state.resume();
        7
    };
    assert_eq!(
        wait_budget(Duration::from_millis(30), Some(&clock), future)
            .await
            .unwrap(),
        7
    );
    assert!(
        wait_budget(
            Duration::from_millis(15),
            Some(&clock),
            std::future::pending::<()>()
        )
        .await
        .is_err()
    );
}
