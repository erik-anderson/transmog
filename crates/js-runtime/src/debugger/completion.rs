//! Completion follows only bounded native data descriptors. It never evaluates input.

use super::{Client, DebugEventKind, MAX_SCOPE_CHAIN};
use std::collections::BTreeSet;
use v8::inspector::StringView;

impl Client {
    pub(super) fn complete(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        request: u64,
        expression: &str,
        pause: u64,
        frame: Option<u32>,
    ) {
        let mut roots = Vec::new();
        if let Some(frame) = frame {
            let frames = self.frames.borrow();
            let Some(chain) = frames
                .get(frame as usize)
                .and_then(|frame| frame["scopeChain"].as_array())
            else {
                self.reject(request, "Completion frame is unavailable");
                return;
            };
            if let Some(session) = self.session.borrow().as_ref() {
                for entry in chain.iter().take(MAX_SCOPE_CHAIN) {
                    if let Some(id) = entry["object"]["objectId"].as_str()
                        && let Ok((value, _, _)) =
                            session.unwrap_object(scope, StringView::from(id.as_bytes()))
                    {
                        roots.push(v8::Global::new(scope, value));
                    }
                }
            }
        } else {
            let context = v8::Local::new(scope, &self.context);
            let global: v8::Local<v8::Value> = context.global(scope).into();
            roots.push(v8::Global::new(scope, global));
        }
        let (matches, omitted) = names(scope, &roots, expression);
        self.emit(
            request,
            DebugEventKind::Completions {
                pause,
                matches,
                omitted,
            },
        );
    }
}

fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn names(
    scope: &mut v8::PinScope<'_, '_>,
    roots: &[v8::Global<v8::Value>],
    expression: &str,
) -> (Vec<String>, bool) {
    let mut parts = expression.split('.').collect::<Vec<_>>();
    let prefix = parts.pop().unwrap_or_default();
    if (!prefix.is_empty() && !identifier(prefix)) || parts.iter().any(|part| !identifier(part)) {
        return (vec![], false);
    }
    v8::tc_scope!(let scope, scope);
    v8::disallow_javascript_execution_scope!(let scope, scope, v8::OnFailure::ThrowOnFailure);
    let mut objects = roots
        .iter()
        .map(|root| v8::Local::new(scope, root))
        .collect::<Vec<_>>();
    for part in parts {
        let Some(key) = v8::String::new(scope, part) else {
            return (vec![], true);
        };
        let mut found = None;
        for object in objects {
            if let Some(value) = property(scope, object, key) {
                found = Some(value);
                break;
            }
        }
        let Some(value) = found else {
            return (vec![], false);
        };
        objects = vec![value];
    }
    let mut results = BTreeSet::new();
    let mut omitted = false;
    let mut remaining = 1024_u32;
    for value in objects {
        let mut value = value;
        for _ in 0..5 {
            if value.is_proxy() {
                break;
            }
            let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
                break;
            };
            if let Some(keys) = object.get_own_property_names(
                scope,
                v8::GetPropertyNamesArgs {
                    mode: v8::KeyCollectionMode::OwnOnly,
                    property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                    index_filter: v8::IndexFilter::SkipIndices,
                    key_conversion: v8::KeyConversionMode::ConvertToString,
                },
            ) {
                let count = keys.length().min(remaining);
                omitted |= count < keys.length();
                remaining -= count;
                for index in 0..count {
                    let Some(key) = keys
                        .get_index(scope, index)
                        .and_then(|key| v8::Local::<v8::String>::try_from(key).ok())
                    else {
                        continue;
                    };
                    if key.utf8_length(scope) > 256 {
                        continue;
                    }
                    let name = key.to_rust_string_lossy(scope);
                    if name.starts_with(prefix) && identifier(&name) {
                        results.insert(name);
                    }
                    if results.len() > 64 {
                        omitted = true;
                        break;
                    }
                }
            }
            if remaining == 0 || results.len() > 64 {
                break;
            }
            let Some(prototype) = object.get_prototype(scope) else {
                break;
            };
            value = prototype;
        }
        if remaining == 0 || results.len() > 64 {
            break;
        }
    }
    (results.into_iter().take(64).collect(), omitted)
}

fn property<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    mut value: v8::Local<'s, v8::Value>,
    key: v8::Local<'s, v8::String>,
) -> Option<v8::Local<'s, v8::Value>> {
    for _ in 0..5 {
        if value.is_proxy() {
            return None;
        }
        let object = v8::Local::<v8::Object>::try_from(value).ok()?;
        if object.has_own_property(scope, key.into())? {
            let descriptor = object.get_own_property_descriptor(scope, key.into())?;
            // An accessor shadows outer/prototype values too; do not invoke it.
            return super::values::data_value(scope, descriptor)
                .or_else(|| Some(v8::undefined(scope).into()));
        }
        value = object.get_prototype(scope)?;
    }
    None
}
