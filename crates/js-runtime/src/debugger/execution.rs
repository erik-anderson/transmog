//! Typed execution controls and bounded, side-effect-checked logpoints.

use super::{Client, DebugEventKind, SourceLocation, Value, console_value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use v8::inspector::StringView;
// Each message also needs a budget-start frame. Leave reserved control capacity
// for pauses, command replies, and callback completion when output is congested.
const LOGPOINTS_PER_SECOND: u32 = 8;

impl Client {
    pub(super) fn restore_pause_mode(&self) {
        let skip = self.options.borrow().skip_pauses;
        let _ = self.dispatch("Debugger.setSkipAllPauses", json!({"skip":skip}));
        if self.pause_next.get()
            && !skip
            && let Some(session) = self.session.borrow().as_ref()
        {
            session.schedule_pause_on_next_statement(
                StringView::from(&b"next callback"[..]),
                StringView::from(&b"{}"[..]),
            );
        }
    }
    pub(super) fn skip_internal_pause(&self, raw: &Value) -> bool {
        if !self.pause_next.get()
            || matches!(
                raw["reason"].as_str(),
                Some("exception" | "promiseRejection")
            )
            || self
                .location(&raw["callFrames"][0]["location"])
                .is_some_and(|location| location.source.starts_with("file:///"))
        {
            return false;
        }
        // SourceURL text never makes an invocation wrapper an authored callback.
        self.internal_resume.set(true);
        if self.dispatch("Debugger.stepInto", json!({})).is_ok() {
            return true;
        }
        self.internal_resume.set(false);
        false
    }
    pub(super) fn controls(&self, request: u64) {
        self.emit(
            request,
            DebugEventKind::Control {
                skip_pauses: self.options.borrow().skip_pauses,
                pause_next: self.pause_next.get(),
            },
        );
    }
    pub(super) fn set_skip(&self, request: u64, skip: bool) {
        if self
            .dispatch("Debugger.setSkipAllPauses", json!({"skip":skip}))
            .is_err()
        {
            self.reject(request, "Pause settings could not be changed");
            return;
        }
        self.options.borrow_mut().skip_pauses = skip;
        if skip {
            if let Some(session) = self.session.borrow().as_ref() {
                session.cancel_pause_on_next_statement();
            }
            self.pause_next.set(false);
        }
        self.controls(request);
    }
    pub(super) fn schedule_pause(&self, request: u64, enabled: bool) {
        if enabled && (self.paused.get() || self.options.borrow().skip_pauses) {
            self.reject(
                request,
                "Enable pauses and continue before scheduling a callback pause",
            );
            return;
        }
        let Some(session) = self.session.borrow().clone() else {
            self.reject(request, "Debugger is unavailable");
            return;
        };
        if enabled {
            session.schedule_pause_on_next_statement(
                StringView::from(&b"next callback"[..]),
                StringView::from(&b"{}"[..]),
            );
        } else {
            session.cancel_pause_on_next_statement();
        }
        self.pause_next.set(enabled);
        self.controls(request);
    }
    pub(super) fn continue_to(&self, request: u64, pause: u64, location: &SourceLocation) {
        if !self.current_pause(pause) {
            self.reject(request, "Pause has expired or resumed");
            return;
        }
        if self.options.borrow().skip_pauses {
            self.reject(request, "Enable pauses before continuing to a location");
            return;
        }
        let id = self
            .sources
            .borrow()
            .iter()
            .find(|(_, source)| source.source == location.source)
            .map(|(id, _)| *id);
        let Some(id) = id else {
            self.reject(request, "This source is not loaded in the selected worker");
            return;
        };
        if self.dispatch("Debugger.continueToLocation", json!({"location":{"scriptId":id.to_string(),"lineNumber":location.line-1,"columnNumber":location.column-1},"targetCallFrames":"current"})).is_ok() {
            self.stepping.set(true);
            self.emit(request, DebugEventKind::Acknowledged);
        } else { self.reject(request, "This location cannot be reached from the current pause"); }
    }
    pub(super) fn logpoint(&self, pause: &Value, frame: &str, expression: &str) {
        self.flush_logpoints();
        if self.logpoint_count.get() >= LOGPOINTS_PER_SECOND {
            self.logpoint_dropped
                .set(self.logpoint_dropped.get().saturating_add(1));
            return;
        }
        self.logpoint_count.set(self.logpoint_count.get() + 1);
        self.emit(0, DebugEventKind::LogpointStarted);
        let result = self.dispatch("Debugger.evaluateOnCallFrame", json!({"callFrameId":frame,"expression":expression,"throwOnSideEffect":true,"silent":true,"returnByValue":false,"timeout":50,"objectGroup":"transmog-logpoint"}));
        let (value, thrown) = console_value(result);
        let _ = self.dispatch(
            "Runtime.releaseObjectGroup",
            json!({"objectGroup":"transmog-logpoint"}),
        );
        self.emit(
            0,
            DebugEventKind::Logpoint {
                location: self.location(&pause["callFrames"][0]["location"]),
                original: None,
                text: value.display,
                thrown,
                omitted: 0,
                time_unix_ms: now(),
            },
        );
    }
    pub(super) fn flush_logpoints(&self) {
        if self.logpoint_epoch.get().elapsed() < Duration::from_secs(1) {
            return;
        }
        self.logpoint_epoch.set(Instant::now());
        self.logpoint_count.set(0);
        self.report_logpoint_loss();
    }
    pub(super) fn report_logpoint_loss(&self) {
        let omitted = self.logpoint_dropped.replace(0);
        if omitted > 0 {
            self.emit(
                0,
                DebugEventKind::Logpoint {
                    location: None,
                    original: None,
                    text: format!("{omitted} logpoint messages omitted"),
                    thrown: false,
                    omitted,
                    time_unix_ms: now(),
                },
            );
        }
    }
}
fn now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(9_007_199_254_740_991)
    .min(9_007_199_254_740_991)
}
