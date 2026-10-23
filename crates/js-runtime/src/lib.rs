#![deny(unsafe_code)]

//! Direct V8 embedding for the isolated script and extension processes.
//!
//! This crate installs no system, network, console, or runtime ops. Callers own
//! package validation, bootstrap restrictions and the external process watchdog.

#[allow(unsafe_code)] // Only V8-provided callback arguments may create CallbackScopes.
mod callbacks;

use std::{cell::RefCell, collections::BTreeMap, fmt, rc::Rc, sync::Once};
use url::Url;

const ERROR_BYTES: usize = 2048;

/// Process-global engine initialization, including string-code restrictions.
/// Safe to call repeatedly and from multiple host entry points.
pub fn initialize() {
    static INITIALIZE: Once = Once::new();
    INITIALIZE.call_once(|| {
        // Retain the previous embedding's language features. Extension bootstrap
        // still removes queueMicrotask; traffic scripts retain its native API.
        v8::V8::set_flags_from_string(concat!(
            "--disallow-code-generation-from-strings",
            " --harmony-temporal --js-float16array --js-explicit-resource-management",
            " --js-source-phase-imports --js-defer-import-eval --enable-queue-microtask",
        ));
        let platform = v8::new_default_platform(4, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// The host-visible stage of an engine failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// Invalid engine configuration or bootstrap failure.
    Initialization,
    /// A module could not compile or link.
    Module,
    /// JavaScript threw or module evaluation could not finish.
    Exception,
    /// The result was not a string.
    NotString,
    /// The UTF-8 result exceeded the host's output budget.
    OutputLimit,
}

/// Bounded engine diagnostics. Hosts decide whether these may reach the user.
#[derive(Debug)]
pub struct Error {
    /// Stage of failure, independent of either host's wire protocol.
    pub kind: ErrorKind,
    message: String,
}

impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

#[derive(Default)]
struct State {
    modules: RefCell<BTreeMap<Url, v8::Global<v8::Module>>>,
    entry: RefCell<Option<Url>>,
    rejected: RefCell<Vec<(v8::Global<v8::Promise>, v8::Global<v8::Value>)>>,
    uncaught: RefCell<Option<v8::Global<v8::Value>>>,
}

/// A single-threaded, capability-free JavaScript context.
pub struct Runtime {
    // Persistent handles must be dropped before their isolate.
    context: v8::Global<v8::Context>,
    isolate: v8::OwnedIsolate,
}

impl Runtime {
    /// Creates the engine with explicit microtasks and bounded V8 heap growth.
    /// External process limits remain required for native/ArrayBuffer memory.
    ///
    /// # Errors
    /// Returns an initialization error for invalid heap limits.
    pub fn new(initial_heap_bytes: usize, max_heap_bytes: usize) -> Result<Self, Error> {
        if initial_heap_bytes == 0 || initial_heap_bytes > max_heap_bytes {
            return Err(Error::new(
                ErrorKind::Initialization,
                "Invalid JavaScript heap limits",
            ));
        }
        initialize();
        let mut isolate = v8::Isolate::new(
            v8::CreateParams::default().heap_limits(initial_heap_bytes, max_heap_bytes),
        );
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        isolate.set_capture_stack_trace_for_uncaught_exceptions(true, 10);
        isolate.set_slot(Rc::new(State::default()));
        isolate.set_promise_reject_callback(callbacks::promise_rejected);
        isolate.add_message_listener(callbacks::uncaught_exception);
        isolate.set_host_initialize_import_meta_object_callback(callbacks::import_meta);
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, v8::ContextOptions::default());
            context.set_allow_generation_from_strings(false);
            v8::Global::new(scope, context)
        };
        Ok(Self { context, isolate })
    }

    /// Executes trusted host bootstrap code before any user module initializer.
    /// Does not run a microtask checkpoint.
    ///
    /// # Errors
    /// Returns a bounded compile or execution error.
    pub fn execute_script(&mut self, name: &str, source: &str) -> Result<(), Error> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);
        run_script(scope, name, source).map(|_| ())
    }

    /// Compiles a complete in-memory graph, links only its supplied modules and
    /// evaluates the entry, including Promise-resolved top-level await.
    ///
    /// # Errors
    /// Returns compilation/linking, evaluation, unhandled rejection or stalled
    /// top-level-await errors. No filesystem or network resolution is performed.
    pub fn evaluate_modules(
        &mut self,
        entry: &Url,
        modules: BTreeMap<Url, String>,
    ) -> Result<(), Error> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);
        let state = state(scope);
        if state.entry.borrow().is_some() {
            return Err(Error::new(
                ErrorKind::Module,
                "Module graph is already initialized",
            ));
        }
        *state.entry.borrow_mut() = Some(entry.clone());
        for (name, code) in modules {
            let source = string(scope, &code)?;
            let origin = origin(scope, name.as_str(), true)?;
            let mut source = v8::script_compiler::Source::new(source, Some(&origin));
            let Some(module) = v8::script_compiler::compile_module(scope, &mut source) else {
                return Err(caught(scope, ErrorKind::Module));
            };
            state
                .modules
                .borrow_mut()
                .insert(name, v8::Global::new(scope, module));
        }
        let module = {
            let modules = state.modules.borrow();
            let module = modules
                .get(entry)
                .ok_or_else(|| Error::new(ErrorKind::Module, "Missing entry module"))?;
            v8::Local::new(scope, module)
        };
        if module.instantiate_module(scope, callbacks::resolve_module) != Some(true) {
            return Err(caught(scope, ErrorKind::Module));
        }
        let Some(value) = module.evaluate(scope) else {
            return Err(caught(scope, ErrorKind::Exception));
        };
        let promise = v8::Local::<v8::Promise>::try_from(value)
            .map_err(|_| Error::new(ErrorKind::Exception, "Invalid module evaluation result"))?;
        // The evaluation promise is observed here, independently of user promises.
        promise.mark_as_handled();
        state
            .rejected
            .borrow_mut()
            .retain(|(pending, _)| pending != &promise);
        pump_tasks(scope);
        scope.perform_microtask_checkpoint();
        match promise.state() {
            v8::PromiseState::Rejected => {
                let exception = promise.result(scope);
                return Err(exception_error(scope, exception));
            }
            v8::PromiseState::Pending => {
                return Err(Error::new(
                    ErrorKind::Exception,
                    "Top-level await did not resolve",
                ));
            }
            v8::PromiseState::Fulfilled => {}
        }
        check_rejections(scope)
    }

    /// Runs ready V8 foreground tasks and a microtask checkpoint before the next
    /// extension operation. Traffic scripts deliberately do not call this.
    ///
    /// # Errors
    /// Returns an unhandled Promise rejection from the preceding operation.
    pub fn poll_tasks(&mut self) -> Result<(), Error> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);
        pump_tasks(scope);
        scope.perform_microtask_checkpoint();
        check_rejections(scope)
    }

    /// Executes a host invocation and extracts its bounded UTF-8 string result.
    /// Microtasks are left queued until the host explicitly polls them.
    ///
    /// # Errors
    /// Returns an execution, result-type or output-budget failure.
    pub fn execute_string(
        &mut self,
        name: &str,
        source: &str,
        max_bytes: usize,
    ) -> Result<String, Error> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);
        let result = run_script(scope, name, source)?;
        let Some(output) = result.to_string(scope) else {
            return Err(Error::new(
                ErrorKind::NotString,
                "JavaScript output is not a string",
            ));
        };
        if output.length() > max_bytes || output.utf8_length(scope) > max_bytes {
            return Err(Error::new(
                ErrorKind::OutputLimit,
                "JavaScript output exceeds its byte limit",
            ));
        }
        Ok(output.to_rust_string_lossy(scope))
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // Clear module and rejection handles while the isolate is still alive.
        // OwnedIsolate then notifies its default platform and discards queued tasks.
        self.isolate.remove_slot::<Rc<State>>();
    }
}

fn state(scope: &v8::Isolate) -> Rc<State> {
    Rc::clone(
        scope
            .get_slot::<Rc<State>>()
            .expect("Runtime owns its isolate state"),
    )
}

fn string<'s>(
    scope: &v8::PinScope<'s, '_>,
    value: &str,
) -> Result<v8::Local<'s, v8::String>, Error> {
    v8::String::new(scope, value).ok_or_else(|| {
        Error::new(
            ErrorKind::Exception,
            "JavaScript string exceeds engine limits",
        )
    })
}

fn origin<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: &str,
    module: bool,
) -> Result<v8::ScriptOrigin<'s>, Error> {
    let name = string(scope, name)?;
    Ok(v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        module,
        None,
    ))
}

type CatchScope<'a, 'b, 's, 'i> = v8::PinnedRef<'a, v8::TryCatch<'b, 's, v8::HandleScope<'i>>>;

fn run_script<'s>(
    scope: &mut CatchScope<'_, '_, 's, '_>,
    name: &str,
    source: &str,
) -> Result<v8::Local<'s, v8::Value>, Error> {
    let source = string(scope, source)?;
    let origin = origin(scope, name, false)?;
    let Some(script) = v8::Script::compile(scope, source, Some(&origin)) else {
        return Err(caught(scope, ErrorKind::Exception));
    };
    script
        .run(scope)
        .ok_or_else(|| caught(scope, ErrorKind::Exception))
}

fn caught(scope: &mut CatchScope<'_, '_, '_, '_>, kind: ErrorKind) -> Error {
    if let Some(value) = scope.exception() {
        let mut error = exception_error(scope, value);
        error.kind = kind;
        error
    } else {
        Error::new(kind, "JavaScript execution failed")
    }
}

fn exception_error(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> Error {
    // Nested TryCatch contains hostile toString/stack access during diagnostics.
    v8::tc_scope!(let scope, scope);
    let stack = v8::Local::<v8::Object>::try_from(value)
        .ok()
        .and_then(|object| {
            let key = v8::String::new(scope, "stack")?;
            let stack = object.get(scope, key.into())?;
            v8::Local::<v8::String>::try_from(stack).ok()
        });
    if let Some(stack) = stack {
        return Error::new(ErrorKind::Exception, bounded_string(scope, stack));
    }
    scope.reset();
    let text = value.to_string(scope).map_or_else(
        || "JavaScript execution failed".to_owned(),
        |value| bounded_string(scope, value),
    );
    Error::new(ErrorKind::Exception, text)
}

fn bounded_string(scope: &v8::Isolate, value: v8::Local<v8::String>) -> String {
    let mut bytes = [0; ERROR_BYTES];
    let written = value.write_utf8_v2(scope, &mut bytes, v8::WriteFlags::kReplaceInvalidUtf8, None);
    String::from_utf8_lossy(&bytes[..written]).into_owned()
}

fn check_rejections(scope: &mut v8::PinScope<'_, '_>) -> Result<(), Error> {
    let state = state(scope);
    let uncaught = state.uncaught.borrow_mut().take();
    let rejected = std::mem::take(&mut *state.rejected.borrow_mut());
    if let Some(error) = uncaught {
        let error = v8::Local::new(scope, error);
        return Err(exception_error(scope, error));
    }
    if let Some((_, error)) = rejected.first() {
        let error = v8::Local::new(scope, error);
        return Err(exception_error(scope, error));
    }
    Ok(())
}

fn pump_tasks(scope: &v8::Isolate) {
    while v8::Platform::pump_message_loop(&v8::V8::get_current_platform(), scope, false) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Runtime {
        Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap()
    }

    fn evaluate(runtime: &mut Runtime, source: &str) -> Result<(), Error> {
        let entry = Url::parse("file:///transmog-script/main.js").unwrap();
        runtime.evaluate_modules(&entry, BTreeMap::from([(entry.clone(), source.to_owned())]))
    }

    #[test]
    fn metadata_and_explicit_microtasks_survive_direct_embedding() {
        let mut runtime = runtime();
        evaluate(&mut runtime, "const value = await Promise.resolve(7); globalThis.value = value; globalThis.meta = import.meta;").unwrap();
        assert_eq!(
            runtime
                .execute_string(
                    "test",
                    "`${meta.url}:${meta.main}:${meta.resolve('./part.js')}:${value}`",
                    200
                )
                .unwrap(),
            "file:///transmog-script/main.js:true:file:///transmog-script/part.js:7"
        );
        runtime
            .execute_script("test", "Promise.resolve().then(() => value++);")
            .unwrap();
        assert_eq!(
            runtime.execute_string("test", "String(value)", 10).unwrap(),
            "7"
        );
        runtime.poll_tasks().unwrap();
        assert_eq!(
            runtime.execute_string("test", "String(value)", 10).unwrap(),
            "8"
        );
        assert_eq!(runtime.execute_string("test", "meta.filename.replaceAll('\\\\', '/') + ':' + meta.dirname.replaceAll('\\\\', '/')", 200).unwrap(),
            "/transmog-script/main.js:/transmog-script");
    }

    #[test]
    fn static_import_metadata_is_ignored_but_other_module_types_are_rejected() {
        for (attribute, accepted) in [
            ("foo:'bar'", true),
            ("type:'json'", false),
            ("type:'text'", false),
            ("type:'javascript'", false),
        ] {
            let mut runtime = runtime();
            let entry = Url::parse("file:///transmog-extension/main.js").unwrap();
            let dependency = entry.join("./helper.js").unwrap();
            let source = format!(
                "import {{title}} from './helper.js' with {{{attribute}}}; globalThis.title = title;"
            );
            let result = runtime.evaluate_modules(
                &entry,
                BTreeMap::from([
                    (entry.clone(), source),
                    (dependency, "export const title = 'ready';".to_owned()),
                ]),
            );
            assert_eq!(result.is_ok(), accepted, "{attribute}: {result:?}");
            if accepted {
                assert_eq!(
                    runtime.execute_string("test", "title", 20).unwrap(),
                    "ready"
                );
            }
        }
    }

    #[test]
    fn retained_language_features_and_native_microtasks_keep_their_contracts() {
        {
            let mut runtime = runtime();
            evaluate(&mut runtime, "globalThis.ready = 0; queueMicrotask(() => ready++); { using resource = { [Symbol.dispose]() { ready++; } }; }").unwrap();
            assert_eq!(
                runtime
                    .execute_string(
                        "test",
                        "`${ready}:${typeof Temporal}:${typeof Float16Array}`",
                        100
                    )
                    .unwrap(),
                "2:object:function"
            );
            runtime
                .execute_script("test", "queueMicrotask(() => ready++);")
                .unwrap();
            assert_eq!(
                runtime.execute_string("test", "String(ready)", 10).unwrap(),
                "2"
            );
            runtime.poll_tasks().unwrap();
            assert_eq!(
                runtime.execute_string("test", "String(ready)", 10).unwrap(),
                "3"
            );
            runtime
                .execute_script(
                    "test",
                    "queueMicrotask(() => {throw new Error('later microtask')});",
                )
                .unwrap();
            assert!(
                runtime
                    .poll_tasks()
                    .unwrap_err()
                    .to_string()
                    .contains("later microtask")
            );
        }
        let mut runtime = runtime();
        let error = evaluate(
            &mut runtime,
            "queueMicrotask(() => {throw new Error('initial microtask')});",
        )
        .unwrap_err();
        assert!(error.to_string().contains("initial microtask"));
        assert!(
            error
                .to_string()
                .contains("file:///transmog-script/main.js:1:")
        );
    }

    #[test]
    fn initialization_and_later_rejections_are_reported_but_handled_ones_are_allowed() {
        {
            let mut runtime = runtime();
            evaluate(
                &mut runtime,
                "Promise.reject(new Error('handled')).catch(() => {});",
            )
            .unwrap();
            runtime
                .execute_script("test", "Promise.reject(new Error('unhandled'));")
                .unwrap();
            assert!(
                runtime
                    .poll_tasks()
                    .unwrap_err()
                    .to_string()
                    .contains("unhandled")
            );
        }
        for source in [
            "await Promise.reject(new Error('rejected'));",
            "Promise.reject(new Error('unhandled'));",
            "await new Promise(() => {});",
        ] {
            let mut runtime = runtime();
            assert_eq!(
                evaluate(&mut runtime, source).unwrap_err().kind,
                ErrorKind::Exception
            );
        }
    }

    #[test]
    fn string_constructors_are_blocked_and_failures_do_not_poison_later_calls() {
        let mut runtime = runtime();
        evaluate(&mut runtime, "globalThis.ready = true;").unwrap();
        for source in [
            "(function(){}).constructor('return 1')()",
            "(async function(){}).constructor('return 1')()",
            "(function*(){}).constructor('return 1')()",
        ] {
            assert!(runtime.execute_string("test", source, 100).is_err());
        }
        assert_eq!(
            runtime.execute_string("test", "String(ready)", 10).unwrap(),
            "true"
        );
    }

    #[test]
    fn utf8_output_is_limited_before_rust_allocation_and_diagnostics_are_bounded() {
        let mut runtime = runtime();
        assert_eq!(
            runtime.execute_string("test", "'éé'", 3).unwrap_err().kind,
            ErrorKind::OutputLimit
        );
        assert_eq!(runtime.execute_string("test", "'éé'", 4).unwrap(), "éé");
        let error = runtime
            .execute_string("test", "throw new Error('x'.repeat(100_000))", 100)
            .unwrap_err();
        assert!(error.to_string().len() <= ERROR_BYTES);
        assert_eq!(
            runtime.execute_string("test", "'ready'", 100).unwrap(),
            "ready"
        );
    }

    #[test]
    fn module_and_invocation_errors_keep_generated_locations_and_contain_hostile_diagnostics() {
        for source in [
            "\nthrow new Error('initializer');",
            "\nawait Promise.reject(new Error('awaited'));",
        ] {
            let mut runtime = runtime();
            let error = evaluate(&mut runtime, source).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("file:///transmog-script/main.js:2:"),
                "{error}"
            );
        }
        let mut runtime = runtime();
        evaluate(
            &mut runtime,
            "\nfunction fail() { throw new Error('handler'); } globalThis.fail = fail;",
        )
        .unwrap();
        let error = runtime.execute_string("test", "fail()", 100).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("file:///transmog-script/main.js:2:"),
            "{error}"
        );
        assert!(
            runtime
                .execute_string(
                    "test",
                    "throw {get stack(){throw 1}, toString(){throw 2}}",
                    100
                )
                .is_err()
        );
        assert_eq!(
            runtime.execute_string("test", "'ready'", 100).unwrap(),
            "ready"
        );
    }
}
