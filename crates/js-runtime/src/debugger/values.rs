//! Pause-local native handles and inert own-descriptor paging.

use std::collections::BTreeMap;
use transmog_script_diagnostics::debugger::{
    DebugEventKind, DebugProperty, DebugValue, MAX_DEBUG_REPLY_BYTES, MAX_OBJECT_HANDLES,
    MAX_PROPERTIES, MAX_VALUE_BYTES,
};
use v8::inspector::{StringView, V8InspectorSession};

#[derive(Default)]
pub(super) struct Values {
    next: u32,
    handles: BTreeMap<u32, v8::Global<v8::Value>>,
}
impl Values {
    pub(super) fn remote(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        session: &V8InspectorSession,
        id: &str,
    ) -> Option<DebugValue> {
        let (value, _, _) = session
            .unwrap_object(scope, StringView::from(id.as_bytes()))
            .ok()?;
        Some(self.preview(scope, value))
    }
    pub(super) fn clear(&mut self) {
        self.handles.clear();
        // Never reuse a released handle within one connection, even after resume.
    }
    pub(super) fn release(&mut self, handles: &[u32]) {
        for handle in handles {
            self.handles.remove(handle);
        }
    }
    fn retain(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        value: v8::Local<v8::Value>,
    ) -> Option<u32> {
        if self.handles.len() >= MAX_OBJECT_HANDLES || value.is_proxy() {
            return None;
        }
        self.next = self.next.checked_add(1)?;
        self.handles
            .insert(self.next, v8::Global::new(scope, value));
        Some(self.next)
    }
    pub(super) fn scope(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        session: &V8InspectorSession,
        id: &str,
    ) -> Option<u32> {
        let (value, _, _) = session
            .unwrap_object(scope, StringView::from(id.as_bytes()))
            .ok()?;
        // Each read owns its handles. An old surface may release its values
        // while its docked/detached replacement is reading the same frame.
        self.retain(scope, value)
    }
    pub(super) fn properties(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        pause: u64,
        handle: u32,
        offset: u32,
    ) -> Option<DebugEventKind> {
        let value = v8::Local::new(scope, self.handles.get(&handle)?);
        if value.is_proxy() {
            return None;
        }
        let object = v8::Local::<v8::Object>::try_from(value).ok()?;
        v8::tc_scope!(let scope,scope);
        // Enumeration/descriptor reads cannot execute script, even for exotic objects.
        v8::disallow_javascript_execution_scope!(let scope,scope,v8::OnFailure::ThrowOnFailure);
        let keys = object.get_own_property_names(
            scope,
            v8::GetPropertyNamesArgs {
                mode: v8::KeyCollectionMode::OwnOnly,
                property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                index_filter: v8::IndexFilter::IncludeIndices,
                key_conversion: v8::KeyConversionMode::ConvertToString,
            },
        )?;
        let end = offset
            .saturating_add(u32::try_from(MAX_PROPERTIES).ok()?)
            .min(keys.length());
        let mut properties = Vec::new();
        let mut bytes = 0;
        let mut next = offset.min(keys.length());
        for index in offset..end {
            let key = keys.get_index(scope, index)?;
            let name = if let Ok(text) = v8::Local::<v8::String>::try_from(key) {
                text_value(scope, text).0
            } else {
                "[Symbol key]".into()
            };
            let descriptor = v8::Local::<v8::Name>::try_from(key)
                .ok()
                .and_then(|key| object.get_own_property_descriptor(scope, key));
            let value = descriptor.and_then(|descriptor| data_value(scope, descriptor));
            // End this page before unavailable object previews. The controller can
            // release its previous page and continue at this exact property.
            if self.handles.len() >= MAX_OBJECT_HANDLES
                && !properties.is_empty()
                && value.is_some_and(|value| value.is_object() && !value.is_proxy())
            {
                break;
            }
            let before = self.next;
            let value = if let Some(value) = value {
                self.preview(scope, value)
            } else {
                unavailable("Accessor or unavailable property; not evaluated")
            };
            let property = DebugProperty { name, value };
            let length = serde_json::to_vec(&property).ok()?.len() + 1;
            if bytes + length > MAX_DEBUG_REPLY_BYTES - 4096 {
                // A speculative preview must not consume a handle when its row is omitted.
                let added = self
                    .handles
                    .range((before.saturating_add(1))..)
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();
                self.release(&added);
                break;
            }
            bytes += length;
            properties.push(property);
            next = index + 1;
        }
        Some(DebugEventKind::Properties {
            pause,
            handle,
            offset,
            properties,
            next: (next < keys.length()).then_some(next),
        })
    }
    fn preview(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        value: v8::Local<v8::Value>,
    ) -> DebugValue {
        let mut output = DebugValue {
            kind: "unavailable".into(),
            display: "Unavailable".into(),
            handle: None,
            truncated: false,
        };
        if value.is_proxy() {
            return unavailable("Proxy: not inspected");
        }
        if value.is_string() || value.is_big_int() {
            output.kind = if value.is_string() {
                "string"
            } else {
                "bigint"
            }
            .into();
            if let Some(text) = value.to_string(scope) {
                (output.display, output.truncated) = text_value(scope, text);
            }
        } else if value.is_number() {
            output.kind = "number".into();
            let number = value.number_value(scope).unwrap_or_default();
            output.display = if number == 0.0 && number.is_sign_negative() {
                "-0".into()
            } else {
                number.to_string()
            };
        } else if value.is_boolean() {
            output.kind = "boolean".into();
            output.display = value.is_true().to_string();
        } else if value.is_null() {
            output.kind = "null".into();
            output.display = "null".into();
        } else if value.is_undefined() {
            output.kind = "undefined".into();
            output.display = "undefined".into();
        } else if value.is_symbol() {
            output.kind = "symbol".into();
            output.display = "[Symbol]".into();
        } else if value.is_object() {
            output.kind = if value.is_function() {
                "function"
            } else if value.is_array() {
                "array"
            } else {
                "object"
            }
            .into();
            output.display = if value.is_function() {
                "[Function]"
            } else if value.is_array() {
                "[Array]"
            } else if value.is_array_buffer_view() {
                "[Typed view]"
            } else {
                "[Object]"
            }
            .into();
            output.handle = self.retain(scope, value);
            if output.handle.is_none() {
                output.display.push_str(" · value handle limit reached");
            }
        }
        output
    }
}
fn unavailable(display: &str) -> DebugValue {
    DebugValue {
        kind: "unavailable".into(),
        display: display.into(),
        handle: None,
        truncated: false,
    }
}
fn text_value(scope: &v8::Isolate, value: v8::Local<v8::String>) -> (String, bool) {
    let mut buffer = [0; MAX_VALUE_BYTES];
    let count = value.write_utf8_v2(
        scope,
        &mut buffer,
        v8::WriteFlags::kReplaceInvalidUtf8,
        None,
    );
    (
        String::from_utf8_lossy(&buffer[..count]).into_owned(),
        count < value.utf8_length(scope),
    )
}
pub(super) fn data_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    descriptor: v8::Local<'s, v8::Value>,
) -> Option<v8::Local<'s, v8::Value>> {
    let descriptor = v8::Local::<v8::Object>::try_from(descriptor).ok()?;
    let key = v8::String::new(scope, "value")?;
    if descriptor.has_own_property(scope, key.into()) != Some(true) {
        return None;
    }
    descriptor.get(scope, key.into())
}
