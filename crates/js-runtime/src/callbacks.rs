//! The only unsafe operations in the embedding: enter V8-provided callback scopes.

use super::{State, state};
use url::Url;

pub(super) fn resolve_module<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    // SAFETY: V8 invokes this callback with the current isolate's live context.
    v8::callback_scope!(unsafe scope, context);
    let state = state(scope);
    let specifier = specifier.to_rust_string_lossy(scope);
    // Unknown metadata attributes were ignored by the previous JavaScript-only
    // loader. A `type` request cannot select a different executable language.
    let javascript = {
        let scope = &mut v8::ContextScope::new(scope, context);
        javascript_attributes(scope, attributes)
    };
    let modules = state.modules.borrow();
    let resolved = modules
        .iter()
        .find(|(_, module)| *module == &referrer)
        .and_then(|(name, _)| name.join(&specifier).ok());
    let result = resolved
        .as_ref()
        .and_then(|name| modules.get(name))
        .filter(|_| javascript)
        .map(|module| v8::Local::new(scope, module));
    drop(modules);
    if result.is_none() {
        throw_type_error(scope, "Module is outside the supplied JavaScript graph");
    }
    result
}

pub(super) extern "C" fn promise_rejected(message: v8::PromiseRejectMessage) {
    // SAFETY: V8 supplies the PromiseRejectMessage for its current isolate.
    v8::callback_scope!(unsafe scope, &message);
    let state = state(scope);
    let promise = message.get_promise();
    match message.get_event() {
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler => {
            if let Some(value) = message.get_value() {
                let promise = v8::Global::new(scope, promise);
                let value = v8::Global::new(scope, value);
                state.rejected.borrow_mut().push((promise, value));
            }
        }
        v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
            state
                .rejected
                .borrow_mut()
                .retain(|(pending, _)| pending != &promise);
        }
        _ => {}
    }
}

pub(super) extern "C" fn uncaught_exception(
    message: v8::Local<v8::Message>,
    exception: v8::Local<v8::Value>,
) {
    // SAFETY: V8 provides its current isolate's live error message and exception.
    v8::callback_scope!(unsafe scope, message);
    let state = state(scope);
    // Native queueMicrotask errors bypass Promise rejection tracking. Record the
    // first without formatting it or executing user getters inside the callback.
    if state.uncaught.borrow().is_none() {
        let exception = v8::Global::new(scope, exception);
        *state.uncaught.borrow_mut() = Some(exception);
    }
}

pub(super) extern "C" fn import_meta(
    context: v8::Local<v8::Context>,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    // SAFETY: V8 supplies the current context, module and metadata object.
    v8::callback_scope!(unsafe scope, context);
    v8::scope!(let scope, scope);
    let state = state(scope);
    let Some(name) = module_name(&state, module) else {
        return;
    };
    let Some(value) = v8::String::new(scope, name.as_str()) else {
        return;
    };
    property(scope, meta, "url", value.into());
    let main = state.entry.borrow().as_ref() == Some(&name);
    let main = v8::Boolean::new(scope, main);
    property(scope, meta, "main", main.into());
    if let Some(resolve) = v8::Function::builder(resolve_meta)
        .data(value.into())
        .build(scope)
    {
        property(scope, meta, "resolve", resolve.into());
    }
    if let Some(path) = metadata_path(&name) {
        if let Some(value) = v8::String::new(scope, &path.display().to_string()) {
            property(scope, meta, "filename", value.into());
        }
        if let Some(parent) = path.parent()
            && let Some(value) = v8::String::new(scope, &parent.display().to_string())
        {
            property(scope, meta, "dirname", value.into());
        }
    }
}

fn metadata_path(name: &Url) -> Option<std::path::PathBuf> {
    if name.scheme() != "file" {
        return None;
    }
    if let Ok(path) = name.to_file_path() {
        return Some(path);
    }
    // Synthetic package URLs have no Windows drive. Retain their decoded path
    // as metadata without ever opening it or consulting the filesystem.
    #[cfg(windows)]
    if name.host_str().is_none() && name.port().is_none() {
        return percent_encoding::percent_decode_str(name.path())
            .decode_utf8()
            .ok()
            .map(|path| std::path::PathBuf::from(path.as_ref()));
    }
    None
}

fn javascript_attributes(
    scope: &mut v8::PinScope<'_, '_>,
    attributes: v8::Local<v8::FixedArray>,
) -> bool {
    // V8 supplies static-import triples: key, value, source offset.
    if !attributes.length().is_multiple_of(3) {
        return false;
    }
    for index in (0..attributes.length()).step_by(3) {
        let Some(key) = attributes
            .get(scope, index)
            .and_then(|key| v8::Local::<v8::String>::try_from(key).ok())
        else {
            return false;
        };
        if key.to_rust_string_lossy(scope) == "type" {
            return false;
        }
    }
    true
}

fn module_name(state: &State, module: v8::Local<v8::Module>) -> Option<Url> {
    state
        .modules
        .borrow()
        .iter()
        .find(|(_, candidate)| *candidate == &module)
        .map(|(name, _)| name.clone())
}

#[allow(clippy::needless_pass_by_value)] // V8's callback ABI requires arguments by value.
fn resolve_meta(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut result: v8::ReturnValue,
) {
    if args.length() > 1 {
        throw_type_error(scope, "Invalid arguments");
        return;
    }
    let Some(specifier) = args.get(0).to_string(scope) else {
        return;
    };
    let specifier = specifier.to_rust_string_lossy(scope);
    let base = args.data().to_rust_string_lossy(scope);
    let resolved = Url::parse(&specifier).or_else(|_| {
        if ["./", "../", "/"]
            .iter()
            .any(|prefix| specifier.starts_with(prefix))
        {
            Url::parse(&base)?.join(&specifier)
        } else {
            Err(url::ParseError::RelativeUrlWithoutBase)
        }
    });
    match resolved {
        Ok(url) => {
            if let Some(value) = v8::String::new(scope, url.as_str()) {
                result.set(value.into());
            }
        }
        Err(_) => throw_type_error(scope, "Invalid module specifier"),
    }
}

fn property(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    name: &str,
    value: v8::Local<v8::Value>,
) {
    if let Some(key) = v8::String::new(scope, name) {
        object.create_data_property(scope, key.into(), value);
    }
}

fn throw_type_error(scope: &mut v8::PinScope, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::type_error(scope, message);
        scope.throw_exception(error);
    }
}
