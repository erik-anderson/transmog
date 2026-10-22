//! FFI ownership and synchronous execution adapters for the application's existing API.
//!
//! Domain operations, input/output schemas, validation, and lifecycle behavior belong
//! to `transmog_app::Application`. This crate adds no proxy or HTTP-client model.
#![deny(unsafe_op_in_unsafe_fn)]

mod abi;
mod body;
mod models;
pub use body::*;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use abi::{FromAbi, Model, Projected};
use tokio::runtime::Runtime;
use transmog_app::{AppConfig, AppError, Application, ErrorCategory};

const MAX_INPUT_BYTES: usize = 256 * 1024;

struct State {
    application: Application,
    runtime: Arc<Runtime>,
}

/// Owns one application's executor and shared application implementation.
pub struct ApplicationHandle {
    state: Option<State>,
    error: Option<AppError>,
}

/// Owns an existing Rust model and its generated ABI projection, or AppError.
pub struct OperationResult {
    value: Result<ErasedModel, Projected<AppError>>,
}

struct ErasedModel {
    kind: u32,
    view: *const u8,
    _owner: Box<dyn std::any::Any>,
}

/// Owns a bounded application hint subscription and its delivery worker.
pub struct SessionUpdatesHandle {
    state: Option<SessionUpdatesState>,
    error: Option<AppError>,
}

struct SessionUpdatesState {
    // Keep the executor alive even if the application owner is released first.
    _runtime: Arc<Runtime>,
    cancel: tokio::sync::watch::Sender<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SessionUpdatesState {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(worker) = self.worker.take() {
            // A callback may release its own subscription. Its current invocation
            // may finish, but no further callbacks will start.
            if worker.thread().id() != std::thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

fn error(category: ErrorCategory, message: &str) -> AppError {
    AppError {
        category,
        message: message.into(),
        retryable: false,
    }
}

fn internal(message: &str) -> AppError {
    error(ErrorCategory::Internal, message)
}

fn contain<T>(operation: impl FnOnce() -> Result<T, AppError>) -> Result<T, AppError> {
    catch_unwind(AssertUnwindSafe(operation))
        .unwrap_or_else(|_| Err(internal("native application operation panicked")))
}

fn result<T: Model>(operation: impl FnOnce() -> Result<T, AppError>) -> *mut OperationResult {
    let value = contain(|| {
        let owner = Box::new(Projected::new(operation()?)?);
        let view = (&raw const owner.view).cast();
        Ok(ErasedModel {
            kind: T::KIND,
            view,
            _owner: owner,
        })
    })
    .map_err(|error| Projected::new(error).expect("AppError projection is infallible"));
    Box::into_raw(Box::new(OperationResult { value }))
}

/// Shared Rust defaults for proxy startup, without a second defaults table.
pub fn default_proxy_start_request() -> *mut OperationResult {
    result(|| Ok(transmog_app::ProxyStartRequest::default()))
}

/// Shared Rust defaults for the complete session query.
pub fn default_session_query_input() -> *mut OperationResult {
    result(|| Ok(transmog_app::SessionQueryInput::default()))
}

/// Private layout handshake, checked by C++ before it exposes generated views.
pub fn model_layout(index: usize) -> usize {
    models::layout(index)
}

/// Creates an application with the same default AppConfig used by Rust callers.
pub fn application_create() -> *mut ApplicationHandle {
    create_application(|| Ok(AppConfig::default()))
}

/// Creates an application with the product body store at a caller-owned cache path.
///
/// # Safety
/// The `length` UTF-8 path bytes must remain readable throughout the call.
pub unsafe fn application_create_with_body_store(
    pointer: *const u8,
    length: usize,
) -> *mut ApplicationHandle {
    create_application(|| {
        let path = std::str::from_utf8(unsafe { input(pointer, length) }?)
            .map_err(|_| error(ErrorCategory::InvalidInput, "body cache path must be UTF-8"))?;
        if path.is_empty() {
            return Err(error(
                ErrorCategory::InvalidInput,
                "body cache path is empty",
            ));
        }
        Ok(AppConfig {
            body_store: Some(transmog_app::BodyStoreConfig::product_default(path.into())),
            ..AppConfig::default()
        })
    })
}

fn create_application(
    config: impl FnOnce() -> Result<AppConfig, AppError>,
) -> *mut ApplicationHandle {
    let state = contain(|| {
        let config = config()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .map_err(|_| internal("application executor could not be created"))?;
        let application = runtime.block_on(async { Application::new(config) })?;
        Ok(State {
            application,
            runtime: Arc::new(runtime),
        })
    });
    let handle = match state {
        Ok(state) => ApplicationHandle {
            state: Some(state),
            error: None,
        },
        Err(error) => ApplicationHandle {
            state: None,
            error: Some(error),
        },
    };
    Box::into_raw(Box::new(handle))
}

/// Returns whether application creation succeeded.
///
/// # Safety
/// A non-null handle must be live and owned by the caller throughout the call.
pub unsafe fn application_valid(handle: *const ApplicationHandle) -> i32 {
    unsafe { handle.as_ref() }
        .is_some_and(|value| value.state.is_some())
        .into()
}

/// Releases an application, performing best-effort shutdown first.
/// Explicit `shutdown` is the normal path because its error can be inspected.
///
/// # Safety
/// A non-null handle must be live, exclusively owned, and released exactly once.
/// No operations may be in flight while releasing it.
pub unsafe fn application_release(handle: *mut ApplicationHandle) {
    if !handle.is_null() {
        // No recoverable Rust panic may reach a generated extern-C thunk.
        let _ = contain(|| {
            let mut handle = unsafe { Box::from_raw(handle) };
            if let Some(State {
                application,
                runtime,
            }) = handle.state.take()
            {
                let _ = contain(|| runtime.block_on(application.shutdown()));
                let entered = runtime.enter();
                drop(application);
                drop(entered);
                drop(runtime);
            }
            Ok(())
        });
    }
}

unsafe fn state<'a>(handle: *const ApplicationHandle) -> Result<&'a State, AppError> {
    let handle = unsafe { handle.as_ref() }
        .ok_or_else(|| error(ErrorCategory::InvalidInput, "application handle is null"))?;
    handle.state.as_ref().ok_or_else(|| {
        handle
            .error
            .clone()
            .unwrap_or_else(|| internal("application is unavailable"))
    })
}

unsafe fn input<'a>(pointer: *const u8, length: usize) -> Result<&'a [u8], AppError> {
    if length > MAX_INPUT_BYTES {
        return Err(error(
            ErrorCategory::Limit,
            "SDK input exceeds 262144 bytes",
        ));
    }
    if pointer.is_null() {
        return Err(error(
            ErrorCategory::InvalidInput,
            "SDK input pointer is null",
        ));
    }
    Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
}

unsafe fn model_input<T: FromAbi>(pointer: *const u8) -> Result<T, AppError> {
    let pointer = pointer.cast::<T::View>();
    if pointer.is_null() || !pointer.is_aligned() {
        return Err(error(
            ErrorCategory::InvalidInput,
            "SDK model pointer is null or misaligned",
        ));
    }
    let mut budget = abi::InputBudget::default();
    budget.consume(std::mem::size_of::<T::View>())?;
    unsafe { T::read(&*pointer, &mut budget) }
}

macro_rules! no_input {
    ($name:ident, $app:ident, $body:block) => {
        /// Calls the corresponding Application operation and owns its typed result.
        ///
        /// # Safety
        /// The handle must be live throughout the call, with no concurrent release.
        pub unsafe fn $name(handle: *const ApplicationHandle) -> *mut OperationResult {
            result(|| {
                let state = unsafe { state(handle) }?;
                let $app = &state.application;
                state.runtime.block_on(async $body)
            })
        }
    };
}

no_input!(status, app, { Ok(app.status()) });
no_input!(product_state, app, { Ok(app.product_state()) });
no_input!(stop_proxy, app, { app.stop_proxy().await });
no_input!(shutdown, app, { app.shutdown().await });

macro_rules! typed_input {
    ($name:ident, $request:ty, |$app:ident, $request_id:ident| $body:block) => {
        /// Copies a generated ABI view into the canonical Rust request and calls Application.
        ///
        /// # Safety
        /// The application and the aligned model view, including all nonempty
        /// text/array slices reachable from it, must remain readable for this call.
        pub unsafe fn $name(handle: *const ApplicationHandle, pointer: *const u8) -> *mut OperationResult {
            result(|| {
                let state = unsafe { state(handle) }?;
                let $request_id: $request = unsafe { model_input(pointer) }?;
                let $app = &state.application;
                state.runtime.block_on(async $body)
            })
        }
    };
}
typed_input!(
    save_product_state,
    transmog_app::ProductState,
    |app, request| { app.save_product_state(request) }
);
typed_input!(
    start_proxy,
    transmog_app::ProxyStartRequest,
    |app, request| { app.start_proxy(request, None).await }
);
typed_input!(
    query_sessions,
    transmog_app::SessionQueryInput,
    |app, request| { app.query_sessions(request) }
);
typed_input!(
    query_traffic_view,
    transmog_app::SessionQueryInput,
    |app, request| { app.query_traffic_view(&request) }
);

unsafe fn view_id<'a>(pointer: *const u8, length: usize) -> Result<&'a str, AppError> {
    std::str::from_utf8(unsafe { input(pointer, length) }?)
        .map_err(|_| error(ErrorCategory::InvalidInput, "traffic view ID must be UTF-8"))
}

/// Reads current rows in an existing application's traffic ordering.
///
/// # Safety
/// Handle and the `length` input bytes must remain live throughout the call.
pub unsafe fn traffic_view_rows(
    handle: *const ApplicationHandle,
    pointer: *const u8,
    length: usize,
    offset: usize,
    limit: usize,
) -> *mut OperationResult {
    result(|| {
        let state = unsafe { state(handle) }?;
        let id = unsafe { view_id(pointer, length) }?;
        state
            .runtime
            .block_on(async { state.application.traffic_view_rows(id, offset, limit) })
    })
}

/// Releases an application traffic ordering; repeated release is accepted.
///
/// # Safety
/// Handle and the `length` input bytes must remain live throughout the call.
pub unsafe fn release_traffic_view(
    handle: *const ApplicationHandle,
    pointer: *const u8,
    length: usize,
) -> *mut OperationResult {
    result(|| {
        let state = unsafe { state(handle) }?;
        let id = unsafe { view_id(pointer, length) }?;
        state.runtime.block_on(async {
            state.application.release_traffic_view(id);
            Ok(())
        })
    })
}

/// Subscribes to the existing SessionHint model, coalesced over 75 milliseconds.
/// Callbacks run on a dedicated delivery thread, outside the Tokio context.
///
/// # Safety
/// The application must be live for this call. `callback` must be a non-null
/// extern-C function taking (usize context, *const u8 SessionHint view, usize size), contain
/// exceptions, and keep its context valid until subscription release completes.
pub unsafe fn subscribe_session_updates(
    handle: *const ApplicationHandle,
    callback: usize,
    context: usize,
) -> *mut SessionUpdatesHandle {
    let state = contain(|| {
        let state = unsafe { state(handle) }?;
        if callback == 0 {
            return Err(error(
                ErrorCategory::InvalidInput,
                "session update callback is null",
            ));
        }
        let callback: unsafe extern "C" fn(usize, *const u8, usize) =
            unsafe { std::mem::transmute(callback) };
        let mut updates = state.application.subscribe_session_updates()?;
        let runtime = Arc::clone(&state.runtime);
        let worker_runtime = Arc::clone(&runtime);
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let worker = std::thread::Builder::new()
            .name("transmog-session-updates".into())
            .spawn(move || {
                let _ = contain(|| {
                    loop {
                        let hint = worker_runtime.block_on(async {
                            if *cancelled.borrow() { return None; }
                            let mut hint = tokio::select! {
                                biased;
                                _ = cancelled.changed() => return None,
                                hint = updates.recv() => hint.ok()?,
                            };
                            let deadline = tokio::time::Instant::now()
                                + std::time::Duration::from_millis(75);
                            loop {
                                tokio::select! {
                                    biased;
                                    _ = cancelled.changed() => return None,
                                    () = tokio::time::sleep_until(deadline) => break,
                                    next = updates.recv() => match next {
                                        Ok(next) => {
                                            hint.exchange_id = next.exchange_id.or(hint.exchange_id);
                                            hint.sequence = hint.sequence.max(next.sequence);
                                            hint.lagged |= next.lagged;
                                        }
                                        Err(_) => break,
                                    },
                                }
                            }
                            Some(hint)
                        });
                        let Some(hint) = hint else { break; };
                        let hint = Projected::new(hint)?;
                        if *cancelled.borrow() { break; }
                        unsafe { callback(context, (&raw const hint.view).cast(), std::mem::size_of_val(&hint.view)) };
                    }
                    Ok(())
                });
            })
            .map_err(|_| internal("session update delivery worker could not start"))?;
        Ok(SessionUpdatesState {
            _runtime: runtime,
            cancel,
            worker: Some(worker),
        })
    });
    let handle = match state {
        Ok(state) => SessionUpdatesHandle {
            state: Some(state),
            error: None,
        },
        Err(error) => SessionUpdatesHandle {
            state: None,
            error: Some(error),
        },
    };
    Box::into_raw(Box::new(handle))
}

/// Reports whether subscription creation succeeded.
///
/// # Safety
/// A non-null handle must remain live throughout the call.
pub unsafe fn session_updates_valid(handle: *const SessionUpdatesHandle) -> i32 {
    unsafe { handle.as_ref() }
        .is_some_and(|value| value.state.is_some())
        .into()
}

/// Returns the subscription creation result using the existing AppError schema.
///
/// # Safety
/// A non-null handle must remain live throughout the call.
pub unsafe fn session_updates_status(handle: *const SessionUpdatesHandle) -> *mut OperationResult {
    result(|| {
        let handle = unsafe { handle.as_ref() }.ok_or_else(|| {
            error(
                ErrorCategory::InvalidInput,
                "session updates handle is null",
            )
        })?;
        if let Some(error) = &handle.error {
            return Err(error.clone());
        }
        Ok(())
    })
}

/// Stops delivery and waits for an in-flight callback, unless called by that callback.
///
/// # Safety
/// A non-null handle must be exclusively owned and released exactly once.
pub unsafe fn session_updates_release(handle: *mut SessionUpdatesHandle) {
    if !handle.is_null() {
        let _ = contain(|| {
            drop(unsafe { Box::from_raw(handle) });
            Ok(())
        });
    }
}

/// Inspects an exchange through Application::session_detail, using its opaque ID.
///
/// # Safety
/// Handle and input must be live/readable throughout the call.
pub unsafe fn session_detail(
    handle: *const ApplicationHandle,
    pointer: *const u8,
    length: usize,
) -> *mut OperationResult {
    result(|| {
        let state = unsafe { state(handle) }?;
        let id = std::str::from_utf8(unsafe { input(pointer, length) }?)
            .map_err(|_| error(ErrorCategory::InvalidInput, "session ID must be UTF-8"))?;
        state
            .runtime
            .block_on(async { state.application.session_detail(id) })
    })
}

/// Returns whether an operation succeeded.
///
/// # Safety
/// A non-null result must be live until this call returns.
pub unsafe fn succeeded(handle: *const OperationResult) -> i32 {
    unsafe { handle.as_ref() }
        .is_some_and(|value| value.value.is_ok())
        .into()
}

/// Borrows a generated model view; an unexpected kind returns null.
/// Kind AppError reads the failure model. Its fields live until release.
///
/// # Safety
/// The result must remain live during this call and every use of the borrowed view.
pub unsafe fn result_model(handle: *const OperationResult, kind: u32) -> *const u8 {
    match unsafe { handle.as_ref() }.map(|result| &result.value) {
        Some(Ok(value)) if value.kind == kind => value.view,
        Some(Err(error)) if kind == AppError::KIND => (&raw const error.view).cast(),
        _ => std::ptr::null(),
    }
}

/// Releases an operation result exactly once; null is accepted.
///
/// # Safety
/// A non-null result must be exclusively owned, live, and not used again.
pub unsafe fn release(handle: *mut OperationResult) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests;
