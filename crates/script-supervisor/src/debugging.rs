//! Exclusive live ownership of an actual persistent revision worker.
use super::{Arc, DebugSession, ScriptFailure, SupervisedScriptRunner, unavailable};
use transmog_script::{ScriptPrefilter, debugger::DebugAction};

#[derive(Default)]
pub(super) struct Admission {
    exchanges: usize,
    reservation: Option<Reservation>,
}
struct Reservation {
    session: Arc<DebugSession>,
    filter: ScriptPrefilter,
    exchange: Option<String>,
    detaching: bool,
}
impl SupervisedScriptRunner {
    /// Whether an explicit controller currently owns this worker.
    pub fn debugger_attached(&self) -> bool {
        self.debug
            .lock()
            .ok()
            .and_then(|value| value.upgrade())
            .is_some_and(|session| !session.is_closed())
    }
    /// Ends ownership when the applied worker is replaced; never restores activation.
    pub fn end_debugger(&self) {
        let session = self.debug.lock().ok().and_then(|value| value.upgrade());
        if let Some(session) = session {
            session.stop("Applied script worker was replaced or paused");
        }
    }
    /// Reserves the actual idle revision worker for one explicitly selected live exchange.
    /// Existing module globals are preserved. Other admissions fail before queuing work.
    /// # Errors
    /// Rejects outstanding pinned exchanges, an owner, invalid filters or an unavailable worker.
    pub fn arm_debugger(
        self: &Arc<Self>,
        session: &Arc<DebugSession>,
        filter: ScriptPrefilter,
    ) -> Result<(), ScriptFailure> {
        if filter.method.as_ref().is_some_and(|value| {
            value.len() > 32 || !value.is_ascii() || value.chars().any(char::is_control)
        }) || filter.host.as_ref().is_some_and(|value| {
            value.len() > 253 || !value.is_ascii() || value.chars().any(char::is_control)
        }) || filter
            .path_prefix
            .as_ref()
            .is_some_and(|value| value.len() > 8192 || value.chars().any(char::is_control))
        {
            return Err(unavailable("The live debugger filter is invalid"));
        }
        let mut state = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.has_failed() || state.exchanges != 0 || state.reservation.is_some() {
            return Err(unavailable(
                "Script worker is busy or already reserved. Finish its existing exchanges before arming.",
            ));
        }
        if !session.arm() {
            return Err(unavailable("Debugger arming is unavailable"));
        }
        let weak = Arc::downgrade(self);
        let id = session.id().to_owned();
        state.reservation = Some(Reservation {
            session: Arc::clone(session),
            filter,
            exchange: None,
            detaching: false,
        });
        // A closed controller invokes this callback synchronously. Never register it
        // while holding admission, which its cleanup must acquire.
        drop(state);
        session.on_closed(move || {
            if let Some(runner) = weak.upgrade() {
                let mut state = runner
                    .admission
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state
                    .reservation
                    .as_ref()
                    .is_some_and(|value| value.session.id() == id)
                {
                    state.reservation = None;
                }
            }
        });
        let attached = self.attach_debugger(Arc::clone(session));
        if attached.is_err() {
            session.stop("Live debugger could not attach");
        }
        attached
    }
    pub(super) fn open_exchange(&self) -> Result<(), ScriptFailure> {
        let mut state = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.exchanges = state
            .exchanges
            .checked_add(1)
            .ok_or_else(|| unavailable("Script exchange admission limit reached"))?;
        Ok(())
    }
    pub(super) fn close_exchange(&self, exchange: &str, terminal: bool) {
        let mut state = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.exchanges = state.exchanges.saturating_sub(1);
        let owner = state
            .reservation
            .as_mut()
            .filter(|value| value.exchange.as_deref() == Some(exchange));
        let Some(owner) = owner else { return };
        owner.detaching = true;
        let session = Arc::clone(&owner.session);
        drop(state);
        if !terminal || self.has_failed() {
            session.stop("Selected debug exchange was canceled or its worker failed");
        } else if session.command(DebugAction::Detach).is_err() {
            session.stop("Live debugger could not detach safely");
        }
    }
    pub(super) fn select_debug_exchange(
        &self,
        exchange: &str,
        method: &str,
        host: &str,
        path: &str,
    ) -> Result<(), ScriptFailure> {
        let mut state = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(owner) = state.reservation.as_mut() else {
            return Ok(());
        };
        if owner.detaching || owner.session.is_closed() {
            return Err(busy());
        }
        if let Some(selected) = &owner.exchange {
            return if selected == exchange {
                Ok(())
            } else {
                Err(busy())
            };
        }
        let matches = owner
            .filter
            .method
            .as_ref()
            .is_none_or(|value| method.eq_ignore_ascii_case(value))
            && owner
                .filter
                .host
                .as_ref()
                .is_none_or(|value| host.eq_ignore_ascii_case(value))
            && owner
                .filter
                .path_prefix
                .as_ref()
                .is_none_or(|value| path.starts_with(value));
        if !matches || !owner.session.claim_exchange(exchange) {
            return Err(busy());
        }
        owner.exchange = Some(exchange.into());
        Ok(())
    }
    pub(super) fn check_debug_admission(&self, exchange: &str) -> Result<(), ScriptFailure> {
        let state = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .reservation
            .as_ref()
            .is_some_and(|owner| owner.detaching || owner.exchange.as_deref() != Some(exchange))
        {
            return Err(busy());
        }
        Ok(())
    }
    pub(super) fn clock_for_exchange(
        &self,
        exchange: &str,
    ) -> Option<Arc<transmog_script_diagnostics::supervision::PauseClock>> {
        let state = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .reservation
            .as_ref()
            .is_some_and(|owner| owner.detaching || owner.exchange.as_deref() != Some(exchange))
        {
            return None;
        }
        self.debug
            .lock()
            .ok()
            .and_then(|binding| binding.upgrade())
            .filter(|session| !session.is_closed())
            .map(|session| session.clock())
    }
}
fn busy() -> ScriptFailure {
    let mut failure = unavailable(
        "Script worker is reserved for a debugger. This exchange was rejected before entering its queue.",
    );
    failure.category = transmog_script::ScriptFailureCategory::ResourceLimit;
    failure
}
