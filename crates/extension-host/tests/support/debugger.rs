use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use transmog_extension_supervisor::DebugSession;
use transmog_extensions::debugger::{
    DebugAction, DebugEventKind, DebugOptions, ExceptionPolicy, ResumeAction,
};
pub fn session(id: char) -> Arc<DebugSession> {
    DebugSession::new(DebugOptions {
        skip_pauses: false,
        pause_on_start: false,
        persistent: false,
        session: id.to_string().repeat(32),
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    })
    .unwrap()
}
pub async fn wait(
    session: &DebugSession,
    predicate: impl Fn(&DebugEventKind) -> bool,
) -> DebugEventKind {
    let start = Instant::now();
    loop {
        let page = session.page(0);
        if let Some(event) = page
            .events
            .into_iter()
            .rev()
            .find(|event| predicate(&event.event))
        {
            return event.event;
        }
        assert!(!page.closed, "Debugger closed before expected event");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Timed out waiting for debugger event"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
pub async fn pause(session: &DebugSession, after: u64) -> u64 {
    let DebugEventKind::Paused { pause, frames, .. } = wait(
        session,
        |event| matches!(event,DebugEventKind::Paused{pause,..} if *pause>after),
    )
    .await
    else {
        unreachable!()
    };
    assert_ne!(frames, []);
    assert!(frames.iter().any(|frame| frame.location.is_some()));
    pause
}
pub fn resume(session: &DebugSession, pause: u64) {
    session
        .command(DebugAction::Resume {
            pause,
            action: ResumeAction::Continue,
        })
        .unwrap();
}
pub async fn local_seven(session: &DebugSession, pause: u64) {
    session
        .command(DebugAction::Scopes { pause, frame: 0 })
        .unwrap();
    let DebugEventKind::Scopes { scopes, .. } = wait(
        session,
        |event| matches!(event,DebugEventKind::Scopes{pause:epoch,..}if *epoch==pause),
    )
    .await
    else {
        unreachable!()
    };
    for scope in scopes
        .iter()
        .filter(|scope| matches!(scope.kind.as_str(), "local" | "block" | "module"))
    {
        if let Some(handle) = scope.handle {
            session
                .command(DebugAction::Properties {
                    pause,
                    handle,
                    offset: 0,
                })
                .unwrap();
            let DebugEventKind::Properties{properties,..}=wait(session,|event|matches!(event,DebugEventKind::Properties{pause:epoch,handle:id,..}if *epoch==pause&&*id==handle)).await else{unreachable!()};
            if properties
                .iter()
                .any(|property| property.name == "local" && property.value.display == "7")
            {
                return;
            }
        }
    }
    panic!("Expected local value 7 in lexical scope");
}
