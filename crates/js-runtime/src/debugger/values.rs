//! Scope-local native handles and inert own-descriptor browsing.

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
    keys: BTreeMap<u32, v8::Global<v8::Array>>,
}
struct PropertyPage {
    properties: Vec<DebugProperty>,
    total: u32,
    next: Option<u32>,
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
        Some(self.preview(scope, value, true))
    }
    pub(super) fn clear(&mut self) {
        self.handles.clear();
        self.keys.clear();
        // Never reuse a released handle within one connection, even after resume.
    }
    pub(super) fn release(&mut self, handles: &[u32]) {
        for handle in handles {
            self.handles.remove(handle);
            self.keys.remove(handle);
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
        let page = self.read_properties(
            scope,
            handle,
            offset,
            u32::try_from(MAX_PROPERTIES).ok()?,
            false,
            offset == 0,
        )?;
        Some(DebugEventKind::Properties {
            pause,
            handle,
            offset,
            properties: page.properties,
            next: page.next,
        })
    }
    pub(super) fn preview_properties(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        pause: u64,
        handle: u32,
        offset: u32,
        limit: u32,
        refresh: bool,
    ) -> Option<DebugEventKind> {
        let page = self.read_properties(scope, handle, offset, limit, true, refresh)?;
        Some(DebugEventKind::PropertyPreview {
            pause,
            handle,
            offset,
            total: page.total,
            properties: page.properties,
            next: page.next,
        })
    }
    pub(super) fn property(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        pause: u64,
        handle: u32,
        index: u32,
    ) -> Option<DebugEventKind> {
        let page = self.read_properties(scope, handle, index, 1, false, false)?;
        Some(DebugEventKind::Evaluated {
            pause,
            value: page.properties.into_iter().next()?.value,
            thrown: false,
        })
    }
    fn read_properties(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        handle: u32,
        offset: u32,
        limit: u32,
        preview_only: bool,
        refresh: bool,
    ) -> Option<PropertyPage> {
        let value = v8::Local::new(scope, self.handles.get(&handle)?);
        if value.is_proxy() {
            return None;
        }
        let object = v8::Local::<v8::Object>::try_from(value).ok()?;
        v8::tc_scope!(let scope,scope);
        // Enumeration/descriptor reads cannot execute script, even for exotic objects.
        v8::disallow_javascript_execution_scope!(let scope,scope,v8::OnFailure::ThrowOnFailure);
        if refresh {
            self.keys.remove(&handle);
        }
        let keys = if let Some(keys) = self.keys.get(&handle) {
            v8::Local::new(scope, keys)
        } else {
            let keys = object.get_own_property_names(
                scope,
                v8::GetPropertyNamesArgs {
                    mode: v8::KeyCollectionMode::OwnOnly,
                    property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                    index_filter: v8::IndexFilter::IncludeIndices,
                    key_conversion: v8::KeyConversionMode::ConvertToString,
                },
            )?;
            self.keys.insert(handle, v8::Global::new(scope, keys));
            keys
        };
        let end = offset.saturating_add(limit).min(keys.length());
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
            if !preview_only
                && self.handles.len() >= MAX_OBJECT_HANDLES
                && !properties.is_empty()
                && value.is_some_and(|value| value.is_object() && !value.is_proxy())
            {
                break;
            }
            let before = self.next;
            let value = if let Some(value) = value {
                self.preview(scope, value, !preview_only)
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
        Some(PropertyPage {
            total: keys.length(),
            properties,
            next: (next < keys.length()).then_some(next),
        })
    }
    fn preview(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        value: v8::Local<v8::Value>,
        retain: bool,
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
            if retain {
                output.handle = self.retain(scope, value);
            }
            if retain && output.handle.is_none() {
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
