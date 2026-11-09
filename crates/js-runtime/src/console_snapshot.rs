//! Bounded, inert Console argument snapshots with no retained engine handles.
use transmog_script_diagnostics::LogValue;

pub(super) fn capture<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    budget: usize,
) -> Vec<LogValue> {
    // Plain messages keep the original text budget and formatting.
    if !(0..args.length().min(16)).any(|index| args.get(index).is_object()) {
        return vec![];
    }
    v8::tc_scope!(let scope, scope);
    v8::disallow_javascript_execution_scope!(let scope, scope, v8::OnFailure::ThrowOnFailure);
    let mut snapshot = Snapshot {
        values: vec![],
        seen: vec![],
        remaining: budget.min(1536),
    };
    for index in 0..args.length().min(16) {
        if !snapshot.visit(scope, args.get(index), "", None, 0) {
            break;
        }
    }
    if snapshot
        .values
        .iter()
        .filter(|value| value.parent.is_none())
        .count()
        < usize::try_from(args.length()).unwrap_or(usize::MAX)
        && let Some(last) = snapshot.values.last_mut()
    {
        last.truncated = true;
    }
    snapshot.values
}

struct Snapshot<'s> {
    values: Vec<LogValue>,
    seen: Vec<v8::Local<'s, v8::Value>>,
    remaining: usize,
}
impl<'s> Snapshot<'s> {
    fn text(&mut self, text: &str, limit: usize) -> String {
        let mut end = text.len().min(limit).min(self.remaining);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.remaining -= end;
        text[..end].into()
    }
    fn visit(
        &mut self,
        scope: &mut v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
        name: &str,
        parent: Option<u16>,
        depth: usize,
    ) -> bool {
        if self.values.len() >= 64 || self.remaining < 80 {
            return false;
        }
        self.remaining -= 48;
        let name_shortened = name.len() > 96;
        let name = self.text(name, 96);
        let (kind, label) = preview(scope, value);
        let display = self.text(&label, 256);
        let index = u16::try_from(self.values.len()).expect("bounded snapshot");
        self.values.push(LogValue {
            name,
            parent,
            kind: kind.into(),
            truncated: name_shortened || display.len() < label.len(),
            display,
        });
        if kind != "object" && kind != "array" {
            return true;
        }
        if self.seen.iter().any(|old| old.strict_equals(value)) {
            self.values[usize::from(index)].display = "[Repeated reference]".into();
            return true;
        }
        self.seen.push(value);
        if depth >= 4 {
            self.values[usize::from(index)].truncated = true;
            return true;
        }
        let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
            return true;
        };
        let keys = property_keys(scope, value, object, &mut self.values[usize::from(index)]);
        let Some(keys) = keys else {
            self.values[usize::from(index)].truncated = true;
            return true;
        };
        let mut captured = 0;
        for offset in 0..keys.length().min(12) {
            let Some(key) = keys.get_index(scope, offset) else {
                break;
            };
            let name = if key.is_string() {
                primitive(scope, key)
            } else {
                "[Symbol key]".into()
            };
            let descriptor = v8::Local::<v8::Name>::try_from(key)
                .ok()
                .and_then(|key| object.get_own_property_descriptor(scope, key));
            let empty = descriptor.is_some_and(|descriptor| descriptor.is_undefined());
            let data = descriptor.and_then(|descriptor| {
                let descriptor = v8::Local::<v8::Object>::try_from(descriptor).ok()?;
                let key = v8::String::new(scope, "value")?;
                if descriptor.has_own_property(scope, key.into()) != Some(true) {
                    return None;
                }
                descriptor.get(scope, key.into())
            });
            if let Some(data) = data {
                if !self.visit(scope, data, &name, Some(index), depth + 1) {
                    break;
                }
            } else {
                if self.remaining < 128 || self.values.len() >= 64 {
                    break;
                }
                self.remaining -= 128;
                let truncated = name.len() > 64;
                let name = self.text(&name, 64);
                self.values.push(LogValue {
                    name,
                    parent: Some(index),
                    kind: "unavailable".into(),
                    display: if empty {
                        "[Empty slot]"
                    } else {
                        "[Accessor; not evaluated]"
                    }
                    .into(),
                    truncated,
                });
            }
            captured += 1;
        }
        self.values[usize::from(index)].truncated |= captured < keys.length();
        true
    }
}
fn property_keys<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    object: v8::Local<'s, v8::Object>,
    root: &mut LogValue,
) -> Option<v8::Local<'s, v8::Array>> {
    if let Ok(array) = v8::Local::<v8::Array>::try_from(value) {
        root.display = format!("[Array({}); indexed items]", array.length());
        root.truncated |= array.length() > 12;
        let keys = (0..array.length().min(12))
            .filter_map(|offset| v8::String::new(scope, &offset.to_string()).map(Into::into))
            .collect::<Vec<_>>();
        Some(v8::Array::new_with_elements(scope, &keys))
    } else {
        object.get_own_property_names(
            scope,
            v8::GetPropertyNamesArgs {
                mode: v8::KeyCollectionMode::OwnOnly,
                property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                index_filter: v8::IndexFilter::IncludeIndices,
                key_conversion: v8::KeyConversionMode::ConvertToString,
            },
        )
    }
}
fn preview(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> (&'static str, String) {
    if value.is_proxy() {
        ("unavailable", "[Proxy; not evaluated]".into())
    } else if value.is_string() {
        ("string", primitive(scope, value))
    } else if value.is_null() {
        ("null", "null".into())
    } else if value.is_undefined() {
        ("undefined", "undefined".into())
    } else if value.is_boolean() {
        ("boolean", value.is_true().to_string())
    } else if value.is_number() {
        ("number", super::console::number_label(scope, value))
    } else if value.is_big_int() {
        ("bigint", primitive(scope, value))
    } else if value.is_symbol() {
        ("symbol", "[Symbol]".into())
    } else if value.is_function() {
        ("function", "[Function]".into())
    } else if value.is_array_buffer_view() {
        (
            "unavailable",
            "[TypedArray or DataView; not captured]".into(),
        )
    } else if value.is_array() {
        ("array", "[Array]".into())
    } else {
        ("object", "[Object]".into())
    }
}
fn primitive(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> String {
    let Some(text) = value.to_string(scope) else {
        return String::new();
    };
    let mut bytes = [0; 512];
    let size = text.write_utf8_v2(scope, &mut bytes, v8::WriteFlags::kReplaceInvalidUtf8, None);
    String::from_utf8_lossy(&bytes[..size]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};
    #[test]
    fn snapshots_preserve_values_and_never_execute_authored_hooks() {
        let records = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&records);
        let mut runtime = crate::Runtime::new(1024 * 1024, 64 * 1024 * 1024).unwrap();
        runtime
            .enable_console(move |_, event| {
                if let transmog_script_diagnostics::ConsoleEvent::Record(record) = event {
                    captured.borrow_mut().push(record);
                }
                true
            })
            .unwrap();
        runtime
            .execute_script("detached", "console.log({before:1})")
            .unwrap();
        assert_eq!(records.borrow()[0].values.len(), 0);
        crate::state(&runtime.isolate)
            .console
            .borrow_mut()
            .snapshots = true;
        runtime.begin_console(1);
        runtime
            .execute_script(
                "snapshot",
                r"
            globalThis.touches=0;
            const value={headers:{accept:'original'}, get secret(){touches++;throw 0}};
            value.self=value;
            const proxy=new Proxy({}, {ownKeys(){touches++;throw 0}, get(){touches++;throw 0}});
            console.warn('request',value,proxy);
            value.headers.accept='changed';
            const wide={}; for(let i=0;i<200;i++) wide['x'.repeat(800)+i]=value;
            console.log(wide);
            console.log('x'.repeat(1000));
            console.log([, undefined, 4]);
            console.log(Infinity, -Infinity, -0, NaN, 1e21);
            console.log({infinite:Infinity,negativeZero:-0});
        ",
            )
            .unwrap();
        assert_eq!(
            runtime
                .execute_string("verify", "String(touches)", 32)
                .unwrap(),
            "0"
        );
        let records = records.borrow();
        let values = &records[1].values;
        assert!(
            values
                .iter()
                .any(|value| value.name == "accept" && value.display == "original")
        );
        assert!(
            values
                .iter()
                .any(|value| value.display.contains("Repeated reference"))
        );
        assert!(
            values
                .iter()
                .any(|value| value.display.contains("Accessor"))
        );
        assert!(values.iter().any(|value| value.display.contains("Proxy")));
        assert!(
            records
                .iter()
                .all(transmog_script_diagnostics::LogRecord::is_bounded)
        );
        assert!(records[2].values.iter().any(|value| value.truncated));
        assert_eq!(records[3].values.len(), 0);
        assert_eq!(records[3].text.len(), 1000);
        assert!(
            records[4]
                .values
                .iter()
                .any(|value| value.name == "0" && value.display == "[Empty slot]")
        );
        assert!(
            records[4]
                .values
                .iter()
                .any(|value| value.name == "1" && value.kind == "undefined")
        );
        assert_eq!(records[5].text, "Infinity -Infinity -0 NaN 1e+21");
        assert!(
            records[6]
                .values
                .iter()
                .any(|value| value.name == "infinite" && value.display == "Infinity")
        );
        assert!(
            records[6]
                .values
                .iter()
                .any(|value| value.name == "negativeZero" && value.display == "-0")
        );
    }
}
