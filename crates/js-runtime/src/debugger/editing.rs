//! Explicit edits use exact native scope/key identities, never generated assignment paths.

use super::{Client, DebugEventKind, Value, console_value, json, values::EditTarget};
use v8::inspector::StringView;

impl Client {
    pub(super) fn set_value(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        request: u64,
        (frame, handle, index): (u32, u32, u32),
        expression: &str,
    ) {
        let Some(target) = self.values.borrow().edit_target(scope, handle, index) else {
            self.reject(request, "This value is read-only or no longer available");
            return;
        };
        if matches!(target, EditTarget::Binding { frame: owner, .. } if owner != frame) {
            self.reject(request, "The selected frame has changed");
            return;
        }
        let Some(id) = self
            .frames
            .borrow()
            .get(frame as usize)
            .and_then(|frame| frame["callFrameId"].as_str())
            .map(str::to_owned)
        else {
            self.reject(request, "The selected frame is unavailable");
            return;
        };
        // An array wrapper retains property replacements of every JavaScript type,
        // including undefined, BigInt and non-finite numbers, without coercion.
        let source = if matches!(target, EditTarget::Property) {
            format!("[(\n{expression}\n)]")
        } else {
            format!("(\n{expression}\n)")
        };
        let _ = self.dispatch("Debugger.setSkipAllPauses", json!({"skip":true}));
        let result = self.dispatch(
            "Debugger.evaluateOnCallFrame",
            json!({
                "callFrameId":id, "expression":source, "silent":true,
                "throwOnSideEffect":true, "timeout":50, "disableBreaks":true,
                "generatePreview":false, "returnByValue":false, "objectGroup":"transmog-edit"
            }),
        );
        self.restore_pause_mode();
        let outcome = match result {
            Ok(result) if result.get("exceptionDetails").is_none() => match target {
                EditTarget::Binding { number, name, .. } => {
                    let raw = &result["result"];
                    let argument = ["value", "unserializableValue", "objectId"]
                        .into_iter()
                        .filter_map(|key| raw.get(key).map(|value| (key.into(), value.clone())))
                        .collect::<serde_json::Map<String, Value>>();
                    self.dispatch(
                        "Debugger.setVariableValue",
                        json!({
                            "callFrameId":id, "scopeNumber":number,
                            "variableName":name, "newValue":argument
                        }),
                    )
                    .map(|_| ())
                    .map_err(|_| "This binding cannot be changed".to_owned())
                }
                EditTarget::Property => self
                    .replace_property(scope, handle, index, &result)
                    .then_some(())
                    .ok_or_else(|| "This property cannot be changed".to_owned()),
            },
            result => Err(console_value(result).0.display),
        };
        let _ = self.dispatch(
            "Runtime.releaseObjectGroup",
            json!({"objectGroup":"transmog-edit"}),
        );
        match outcome {
            Ok(()) => self.emit(request, DebugEventKind::Acknowledged),
            Err(message) => self.reject(request, &message),
        }
    }

    fn replace_property(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        handle: u32,
        index: u32,
        result: &Value,
    ) -> bool {
        let Some(id) = result["result"]["objectId"].as_str() else {
            return false;
        };
        let Some(session) = self.session.borrow().clone() else {
            return false;
        };
        let Ok((array, _, _)) = session.unwrap_object(scope, StringView::from(id.as_bytes()))
        else {
            return false;
        };
        let Ok(array) = v8::Local::<v8::Array>::try_from(array) else {
            return false;
        };
        v8::tc_scope!(let scope,scope);
        v8::disallow_javascript_execution_scope!(let scope,scope,v8::OnFailure::ThrowOnFailure);
        let Some(value) = array.get_index(scope, 0) else {
            return false;
        };
        self.values
            .borrow()
            .replace_property(scope, handle, index, value)
    }
}
