//! One callback-admission claim per window, independent of Console evaluation.
use super::{AppError, Application, OWNER, State, Workspace, invalid};
use std::sync::Arc;
use transmog_script_diagnostics::{
    debugger::DebugAction,
    supervision::{CallbackGate, Session},
};
impl Workspace {
    pub(super) fn callback_gate(&self) -> CallbackGate {
        let workspace = Arc::downgrade(&self.0);
        Arc::new(move |session: &Session| {
            if session.is_closed() {
                return Ok(());
            }
            let Some(workspace) = workspace.upgrade() else {
                return Ok(());
            };
            let (revision, request) = {
                let mut state = workspace.lock().map_err(|_| "Debugger unavailable")?;
                if !state.open
                    || !state.pause_any
                    || state.pause_claim.is_some()
                    || state.skip_pauses
                {
                    return Ok(());
                }
                // Claim at callback admission, never at worker creation. The shared
                // lock prevents simultaneous workers from consuming the same arm.
                let control = session
                    .wait_attached()
                    .and_then(|()| session.command(DebugAction::SkipPauses { skip: false }))
                    .and_then(|_| session.command(DebugAction::PauseNext { enabled: true }));
                let request = match control {
                    Ok(request) => request,
                    Err(_) if session.detached() => return Ok(()),
                    Err(error) => return Err(error),
                };
                state.pause_claim = Some(session.id().to_owned());
                state.pause_claim_request = request;
                (state.pause_revision, request)
            };
            let result = session.wait_control(request);
            let mut state = workspace.lock().map_err(|_| "Debugger unavailable")?;
            if state.pause_revision != revision {
                let cancel = session.command(DebugAction::PauseNext { enabled: false });
                drop(state);
                if let Ok(request) = cancel {
                    let canceled = session.wait_control(request);
                    if !session.detached() {
                        canceled?;
                    }
                }
            } else if result.is_err() {
                state.pause_claim = None;
            }
            if session.detached() { Ok(()) } else { result }
        })
    }
    pub(super) fn pause_pending(
        state: &mut State,
        debuggers: &crate::debugging::Debuggers,
    ) -> bool {
        if let Some(id) = &state.pause_claim {
            let ended = debuggers.owned(OWNER, id).is_ok_and(|run| {
                let page = run.session.page(u64::MAX);
                page.closed
                    || page.paused.is_some()
                    || (!run.session.control_pending(state.pause_claim_request)
                        && !page.active
                        && !page.controls.pause_next)
            });
            if ended {
                state.pause_any = false;
                state.pause_claim = None;
            }
        }
        if state.pause_target != "any" && !state.pause_target.is_empty() {
            return debuggers
                .owned(OWNER, &state.pause_target)
                .is_ok_and(|run| {
                    let page = run.session.page(u64::MAX);
                    !page.closed && page.controls.pause_next
                });
        }
        state.pause_any
    }
}
impl Application {
    /// Selects and arms one callback pause, including workers which do not exist yet.
    /// # Errors
    /// Rejects foreign/ended targets, closed workspaces, and unavailable controls.
    pub fn set_workspace_pause_next(&self, target: &str, enabled: bool) -> Result<(), AppError> {
        if target != "any" {
            let run = self.debuggers.owned(OWNER, target)?;
            if enabled && run.session.is_closed() {
                return Err(invalid("Selected worker has ended"));
            }
        }
        if enabled {
            self.set_workspace_skip_pauses(false)?;
        }
        let previous = {
            let mut state = self
                .debugger_workspace
                .0
                .lock()
                .map_err(|_| invalid("Debugger unavailable"))?;
            if !state.open {
                return Err(invalid("Debugger is closed"));
            }
            let previous = state.pause_claim.take().or_else(|| {
                (!state.pause_target.is_empty() && state.pause_target != "any")
                    .then(|| state.pause_target.clone())
            });
            state.pause_revision = state.pause_revision.wrapping_add(1);
            state.pause_target = target.into();
            state.pause_any = enabled && target == "any";
            previous
        };
        if let Some(previous) = previous
            && let Ok(run) = self.debuggers.owned(OWNER, &previous)
        {
            let _ = run
                .session
                .command(DebugAction::PauseNext { enabled: false });
        }
        if enabled && target != "any" {
            self.workspace_debug_command(target, DebugAction::PauseNext { enabled: true })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DebugConfiguration, DebugSessionInfo};
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };
    use transmog_script_diagnostics::debugger::{DebugEvent, DebugEventKind};
    fn worker(
        app: &Application,
        count: &Arc<AtomicUsize>,
        hold: &Arc<AtomicBool>,
    ) -> Arc<crate::debugging::Run> {
        let run = app
            .debuggers
            .create(
                OWNER,
                &DebugConfiguration {
                    skip_pauses: false,
                    pause_on_start: false,
                    persistent: true,
                    exceptions: transmog_script_diagnostics::debugger::ExceptionPolicy::None,
                    breakpoints: vec![],
                },
                DebugSessionInfo {
                    session: String::new(),
                    mode: "test".into(),
                    label: "worker".into(),
                    artifact: "test".into(),
                    source: "file:///test.js".into(),
                },
                None,
            )
            .unwrap();
        let session = &run.session;
        let (send, receive) = mpsc::channel();
        session.bind(move |command| send.send(command).is_ok(), || {});
        session
            .accept(DebugEvent {
                value_generation: None,
                session: session.id().into(),
                invocation: 0,
                request: 0,
                event: DebugEventKind::Attached,
            })
            .unwrap();
        let owner = Arc::downgrade(session);
        let count = Arc::clone(count);
        let hold = Arc::clone(hold);
        std::thread::spawn(move || {
            let mut skip = false;
            let mut pause = false;
            while let Ok(command) = receive.recv() {
                let Some(session) = owner.upgrade() else {
                    break;
                };
                match command.action {
                    DebugAction::SkipPauses { skip: value } => {
                        skip = value;
                        if skip {
                            pause = false;
                        }
                    }
                    DebugAction::PauseNext { enabled } => {
                        pause = enabled;
                        if enabled {
                            count.fetch_add(1, Ordering::AcqRel);
                            while hold.load(Ordering::Acquire) {
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }
                    }
                    DebugAction::Detach => {
                        let _ = session.accept(DebugEvent {
                            value_generation: None,
                            session: session.id().into(),
                            invocation: 0,
                            request: command.request,
                            event: DebugEventKind::Detached,
                        });
                        break;
                    }
                    _ => continue,
                }
                let _ = session.accept(DebugEvent {
                    value_generation: None,
                    session: session.id().into(),
                    invocation: 0,
                    request: command.request,
                    event: DebugEventKind::Control {
                        skip_pauses: skip,
                        pause_next: pause,
                    },
                });
            }
        });
        app.debugger_workspace
            .0
            .lock()
            .unwrap()
            .targets
            .push(run.info.clone());
        run
    }
    #[test]
    fn any_target_waits_for_callbacks_claims_once_and_can_be_rearmed() {
        let app = Application::new(crate::AppConfig::default()).unwrap();
        app.open_debugger_workspace().unwrap();
        app.set_workspace_pause_next("any", true).unwrap();
        assert!(app.debugger_workspace().unwrap().pause_pending);
        let count = Arc::new(AtomicUsize::new(0));
        let hold = Arc::new(AtomicBool::new(false));
        let one = worker(&app, &count, &hold);
        let two = worker(&app, &count, &hold);
        assert_eq!(
            count.load(Ordering::Acquire),
            0,
            "Attachment must not consume an arm"
        );
        std::thread::scope(|scope| {
            scope.spawn(|| one.session.begin_ready(1).unwrap());
            scope.spawn(|| two.session.begin_ready(1).unwrap());
        });
        assert_eq!(count.load(Ordering::Acquire), 1);
        one.session.completed();
        two.session.completed();
        app.set_workspace_pause_next("any", false).unwrap();
        app.set_workspace_pause_next("any", true).unwrap();
        two.session.begin_ready(2).unwrap();
        assert_eq!(count.load(Ordering::Acquire), 2);
        two.session.completed();
        app.close_debugger_workspace().unwrap();
        app.open_debugger_workspace().unwrap();
        let view = app.debugger_workspace().unwrap();
        assert_eq!(view.pause_target, "any");
        assert!(!view.pause_pending);
        one.session.stop("test finished");
        two.session.stop("test finished");
    }
    #[test]
    fn a_detached_callback_does_not_consume_a_new_windows_pause_arm() {
        let app = Application::new(crate::AppConfig::default()).unwrap();
        app.open_debugger_workspace().unwrap();
        let run = worker(
            &app,
            &Arc::new(AtomicUsize::new(0)),
            &Arc::new(AtomicBool::new(false)),
        );
        run.session.request_detach().unwrap();
        let start = std::time::Instant::now();
        while !run.session.detached() {
            assert!(start.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(2));
        }
        app.set_workspace_pause_next("any", true).unwrap();
        assert_eq!(run.session.begin_ready(1), Ok(false));
        assert!(app.debugger_workspace().unwrap().pause_pending);
    }
    #[test]
    fn cancellation_during_admission_keeps_the_worker_healthy_and_reset_rejects_live_ids() {
        let app = Application::new(crate::AppConfig::default()).unwrap();
        app.open_debugger_workspace().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let hold = Arc::new(AtomicBool::new(true));
        let run = worker(&app, &count, &hold);
        assert!(app.reset_scratch_console(Some(run.session.id())).is_err());
        app.set_workspace_pause_next("any", true).unwrap();
        std::thread::scope(|scope| {
            let admission = scope.spawn(|| run.session.begin_ready(1));
            let start = std::time::Instant::now();
            while count.load(Ordering::Acquire) == 0 {
                assert!(start.elapsed() < Duration::from_secs(2));
                std::thread::sleep(Duration::from_millis(2));
            }
            app.set_workspace_pause_next("any", false).unwrap();
            hold.store(false, Ordering::Release);
            assert!(admission.join().unwrap().unwrap());
        });
        assert!(!run.session.is_closed());
        assert!(!run.session.page(0).controls.pause_next);
        run.session.completed();
        run.session.stop("test finished");
    }
}
