//! Window-owned Scratch contexts survive debugger surface moves and dismissal.
use super::{AppError, Application, DebugConfiguration, DebugSessionInfo, ErrorCategory, invalid};
use crate::debugging::Run;
use serde::Serialize;
use std::sync::Arc;
use transmog_script_diagnostics::debugger::{DebugAction, ExceptionPolicy};
use transmog_script_supervisor::ScratchRunner;

pub(super) const SCRATCH_OWNER: &str = "scratch-console";
/// Independent Console destination; it never registers traffic handlers or sources.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchContext {
    /// Exact current heap identity. Reset always creates a different identity.
    pub info: DebugSessionInfo,
}
#[derive(Default)]
pub(super) struct Scratch {
    current: Option<(Arc<Run>, ScratchRunner)>,
}
impl Scratch {
    pub(super) fn view(&self) -> Option<ScratchContext> {
        self.current.as_ref().map(|(run, _)| ScratchContext {
            info: run.info.clone(),
        })
    }
    fn clear(&mut self) {
        if let Some((run, _worker)) = self.current.take() {
            run.session.stop("Scratch reset");
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        self.clear();
    }
}
impl Application {
    /// Starts this window's Scratch heap on demand, independently of debugger targets.
    /// # Errors
    /// Rejects a closed debugger, unavailable sandbox, or failed worker startup.
    pub fn scratch_console(&self) -> Result<ScratchContext, AppError> {
        if !self.debugger_workspace.is_open() {
            return Err(invalid("Open the debugger before using Scratch"));
        }
        let mut scratch = self
            .debugger_workspace
            .1
            .lock()
            .map_err(|_| invalid("Scratch unavailable"))?;
        if let Some((run, _)) = &scratch.current {
            if !run.session.is_closed() {
                return Ok(ScratchContext {
                    info: run.info.clone(),
                });
            }
            return Err(invalid(
                "Scratch ended. Choose Reset scratch to start fresh.",
            ));
        }
        let config = self.scripts.host_config()?;
        let run = self.debuggers.create(
            SCRATCH_OWNER,
            &DebugConfiguration {
                skip_pauses: false,
                pause_on_start: false,
                persistent: true,
                exceptions: ExceptionPolicy::None,
                breakpoints: vec![],
            },
            DebugSessionInfo {
                session: String::new(),
                mode: "Scratch".into(),
                label: "Scratch".into(),
                artifact: "scratch".into(),
                source: String::new(),
            },
            None,
        )?;
        let worker = ScratchRunner::start(&config, &run.session).map_err(|failure| {
            run.session.stop("Scratch could not start");
            AppError::new(ErrorCategory::Unavailable, failure.message, true)
        })?;
        let view = ScratchContext {
            info: run.info.clone(),
        };
        scratch.current = Some((run, worker));
        Ok(view)
    }
    /// Resets only Scratch; no live-worker identifier can authorize this operation.
    /// # Errors
    /// Rejects a closed debugger or a stale identity from an earlier Scratch heap.
    pub fn reset_scratch_console(&self, expected: Option<&str>) -> Result<(), AppError> {
        if !self.debugger_workspace.is_open() {
            return Err(invalid("Debugger is closed"));
        }
        let mut scratch = self
            .debugger_workspace
            .1
            .lock()
            .map_err(|_| invalid("Scratch unavailable"))?;
        if expected.is_some_and(|id| {
            scratch
                .current
                .as_ref()
                .is_none_or(|(run, _)| run.info.session != id)
        }) {
            return Err(invalid("Scratch context has changed"));
        }
        scratch.clear();
        Ok(())
    }
    /// Releases a closing owning window's heap, including while its debugger is hidden.
    pub fn release_scratch_console(&self) {
        if let Ok(mut scratch) = self.debugger_workspace.1.lock() {
            scratch.clear();
        }
    }
    pub(super) fn scratch_command(&self, id: &str, action: DebugAction) -> Result<u64, AppError> {
        if !matches!(
            action,
            DebugAction::Evaluate { .. }
                | DebugAction::Inspect { .. }
                | DebugAction::Complete { .. }
                | DebugAction::Properties { .. }
                | DebugAction::PreviewProperties { .. }
                | DebugAction::Property { .. }
                | DebugAction::Release { .. }
        ) {
            return Err(invalid(
                "This control is for script workers. Use Reset scratch for Scratch.",
            ));
        }
        self.debug_command(SCRATCH_OWNER, id, action)
    }
}
