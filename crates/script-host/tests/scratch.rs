//! Scratch executes in the real isolated engine without registering a traffic script.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use transmog_script::debugger::{DebugAction, DebugEventKind, DebugOptions, ExceptionPolicy};
use transmog_script_supervisor::{DebugSession, IsolationPolicy, ScratchRunner, ScriptHostConfig};

fn session(id: char) -> Arc<DebugSession> {
    DebugSession::new(DebugOptions {
        skip_pauses: false,
        pause_on_start: false,
        persistent: true,
        session: id.to_string().repeat(32),
        exceptions: ExceptionPolicy::None,
        breakpoints: vec![],
    })
    .unwrap()
}
fn evaluate(session: &DebugSession, expression: &str) -> (String, bool) {
    let request = session
        .command(DebugAction::Evaluate {
            expression: expression.into(),
            pause: 0,
            frame: None,
        })
        .unwrap();
    let start = Instant::now();
    loop {
        let page = session.page(0);
        for event in page.events {
            if event.request == request
                && let DebugEventKind::Evaluated { value, thrown, .. } = event.event
            {
                return (value.display, thrown);
            }
        }
        assert!(!page.closed, "Scratch ended during evaluation");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Scratch evaluation did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn scratch_keeps_lexical_variables_isolates_heaps_and_bounds_commands() {
    let config = ScriptHostConfig {
        executable: env!("CARGO_BIN_EXE_transmog-script-host").into(),
        isolation: if cfg!(windows) {
            IsolationPolicy::RequireSandbox
        } else {
            IsolationPolicy::DevelopmentOnly
        },
    };
    let first = session('a');
    let first_worker = ScratchRunner::start(&config, &first).unwrap();
    assert_eq!(
        evaluate(
            &first,
            "let answer = 41; const object = { nested: answer }; answer"
        ),
        ("41".into(), false)
    );
    assert_eq!(evaluate(&first, "answer + 1"), ("42".into(), false));
    assert_eq!(
        evaluate(&first, "object.nested === 41"),
        ("true".into(), false)
    );
    assert_eq!(
        evaluate(
            &first,
            "typeof fetch === 'undefined' && typeof process === 'undefined' && typeof document === 'undefined'"
        ),
        ("true".into(), false)
    );
    assert!(evaluate(&first, "while (true) {}").1);
    assert_eq!(evaluate(&first, "answer"), ("41".into(), false));
    let second = session('b');
    let second_worker = ScratchRunner::start(&config, &second).unwrap();
    assert_eq!(
        evaluate(&second, "typeof answer === 'undefined'"),
        ("true".into(), false)
    );
    first.stop("Reset scratch");
    drop(first_worker);
    assert!(first.is_closed());
    assert_eq!(evaluate(&second, "6 * 7"), ("42".into(), false));
    second.stop("Owning window closed");
    drop(second_worker);
}
