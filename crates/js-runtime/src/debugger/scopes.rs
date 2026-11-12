//! Fresh native scope snapshots without changing the controlling debugger session.

use super::{
    Channel, ChannelImpl, Client, DebugEventKind, DebugScope, MAX_INSPECTOR_BYTES, MAX_SCOPE_CHAIN,
    Rc, RefCell, StringBuffer, StringView, V8InspectorClientTrustLevel, Value, json, short,
};

impl Client {
    pub(super) fn scopes(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        request: u64,
        pause: u64,
        frame: u32,
    ) {
        if self.frames.borrow().get(frame as usize).is_none() {
            self.reject(request, "Scopes are unavailable in this frame");
            return;
        }
        // Keep the existing pause owned while the temporary reader disconnects,
        // even if the user has enabled Skip pauses for subsequent execution.
        let skip = self.options.borrow().skip_pauses;
        if skip
            && self
                .dispatch("Debugger.setSkipAllPauses", json!({"skip":false}))
                .is_err()
        {
            self.reject(request, "Scope inspection is unavailable");
            return;
        }
        let scopes = self.scope_snapshot(scope, frame);
        if skip {
            self.restore_pause_mode();
        }
        let Some(scopes) = scopes else {
            self.reject(request, "Current scopes are unavailable in this frame");
            return;
        };
        self.emit(
            request,
            DebugEventKind::Scopes {
                pause,
                frame,
                scopes,
            },
        );
    }

    fn scope_snapshot(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        frame: u32,
    ) -> Option<Vec<DebugScope>> {
        let inspector = self.inspector.upgrade()?;
        // Inspector scope objects are transient copies, not live bindings. Enabling
        // a short-lived reader while paused asks V8 to materialize current scopes.
        // The controlling session keeps its pause, breakpoints and exception policy.
        let snapshot = Rc::new(RefCell::new(None));
        let session = inspector.inspector.connect(
            1,
            Channel::new(Box::new(Snapshot {
                frame,
                snapshot: Rc::clone(&snapshot),
            })),
            StringView::from(&b"{}"[..]),
            V8InspectorClientTrustLevel::Untrusted,
        );
        session.dispatch_protocol_message(StringView::from(
            &b"{\"id\":1,\"method\":\"Debugger.enable\",\"params\":{\"maxScriptsCacheSize\":0}}"[..],
        ));
        let snapshot = snapshot.borrow_mut().take()?;
        let chain = snapshot["scopeChain"].as_array()?;
        let scopes = chain
            .iter()
            .take(MAX_SCOPE_CHAIN)
            .enumerate()
            .map(|(number, entry)| {
                let kind = entry["type"].as_str().unwrap_or("unavailable");
                let handle = entry["object"]["objectId"]
                    .as_str()
                    .filter(|id| id.len() <= 2048)
                    .and_then(|id| {
                        self.values
                            .borrow_mut()
                            .scope(scope, &session, id, (frame, number, kind))
                    });
                DebugScope {
                    kind: short(kind, 64),
                    name: short(entry["name"].as_str().unwrap_or(""), 256),
                    handle,
                }
            })
            .collect();
        Some(scopes)
    }
}

struct Snapshot {
    frame: u32,
    snapshot: Rc<RefCell<Option<Value>>>,
}
impl ChannelImpl for Snapshot {
    fn send_response(&self, _id: i32, _message: v8::UniquePtr<StringBuffer>) {}
    fn send_notification(&self, message: v8::UniquePtr<StringBuffer>) {
        let Some(message) = message.as_ref() else {
            return;
        };
        if message.string().len() > MAX_INSPECTOR_BYTES / 3 {
            return;
        }
        let Ok(event) = serde_json::from_str::<Value>(&message.string().to_string()) else {
            return;
        };
        if event["method"] == "Debugger.paused" {
            *self.snapshot.borrow_mut() = event["params"]["callFrames"]
                .get(self.frame as usize)
                .cloned();
        }
    }
    fn flush_protocol_notifications(&self) {}
}
