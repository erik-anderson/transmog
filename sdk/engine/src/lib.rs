//! FFI ownership and synchronous execution adapters for the application's existing API.
//!
//! Domain operations, input/output schemas, validation, and lifecycle behavior belong
//! to `transmog_app::Application`. This crate adds no proxy or HTTP-client model.
#![deny(unsafe_op_in_unsafe_fn)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use serde::{Serialize, de::DeserializeOwned};
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

/// Owned serialization of one existing application result or error.
pub struct OperationResult {
    json: Vec<u8>,
    error: Vec<u8>,
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

fn result(operation: impl FnOnce() -> Result<Vec<u8>, AppError>) -> *mut OperationResult {
    let value = match contain(operation) {
        Ok(json) => OperationResult {
            json,
            error: Vec::new(),
        },
        Err(error) => OperationResult {
            json: Vec::new(),
            // AppError contains only serializable scalar fields.
            error: serde_json::to_vec(&error).expect("AppError serialization"),
        },
    };
    Box::into_raw(Box::new(value))
}

fn json(value: impl Serialize) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec(&value).map_err(|_| internal("application result could not be serialized"))
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

unsafe fn parse<T: DeserializeOwned>(pointer: *const u8, length: usize) -> Result<T, AppError> {
    serde_json::from_slice(unsafe { input(pointer, length) }?).map_err(|_| {
        error(
            ErrorCategory::InvalidInput,
            "input does not match the application request schema",
        )
    })
}

macro_rules! no_input {
    ($name:ident, $app:ident, $body:block) => {
        /// Calls the corresponding Application operation and owns its serialized result.
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

no_input!(status, app, { json(app.status()) });
no_input!(product_state, app, { json(app.product_state()) });
no_input!(stop_proxy, app, { json(app.stop_proxy().await?) });
no_input!(shutdown, app, {
    app.shutdown().await?;
    json(())
});

macro_rules! json_input {
    ($name:ident, $request:ty, |$app:ident, $request_id:ident| $body:block) => {
        /// Calls the corresponding Application operation with its existing request schema.
        ///
        /// # Safety
        /// The handle must be live and input must reference `length` readable bytes.
        pub unsafe fn $name(handle: *const ApplicationHandle, pointer: *const u8,
                            length: usize) -> *mut OperationResult {
            result(|| {
                let state = unsafe { state(handle) }?;
                let $request_id: $request = unsafe { parse(pointer, length) }?;
                let $app = &state.application;
                state.runtime.block_on(async $body)
            })
        }
    };
}

json_input!(
    save_product_state,
    transmog_app::ProductState,
    |app, request| { json(app.save_product_state(request)?) }
);
json_input!(
    start_proxy,
    transmog_app::ProxyStartRequest,
    |app, request| { json(app.start_proxy(request, None).await?) }
);
json_input!(
    query_sessions,
    transmog_app::SessionQueryInput,
    |app, request| { json(app.query_sessions(request)?) }
);
json_input!(
    read_body_range,
    transmog_app::BodyRangeRequest,
    |app, request| { json(app.read_body_range(request).await?) }
);
json_input!(
    query_traffic_view,
    transmog_app::SessionQueryInput,
    |app, request| { json(app.query_traffic_view(&request)?) }
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
            .block_on(async { json(state.application.traffic_view_rows(id, offset, limit)?) })
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
            json(())
        })
    })
}

/// Subscribes to the existing SessionHint model, coalesced over 75 milliseconds.
/// Callbacks run on a dedicated delivery thread, outside the Tokio context.
///
/// # Safety
/// The application must be live for this call. `callback` must be a non-null
/// extern-C function taking (usize context, *const u8 JSON, usize length), contain
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
                        let bytes = json(hint)?;
                        if *cancelled.borrow() { break; }
                        unsafe { callback(context, bytes.as_ptr(), bytes.len()) };
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
        json(())
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
            .block_on(async { json(state.application.session_detail(id)?) })
    })
}

/// Returns whether an operation succeeded.
///
/// # Safety
/// A non-null result must be live until this call returns.
pub unsafe fn succeeded(handle: *const OperationResult) -> i32 {
    unsafe { handle.as_ref() }
        .is_some_and(|value| value.error.is_empty())
        .into()
}

macro_rules! bytes {
    ($pointer:ident, $length:ident, $field:ident) => {
        /// Borrows serialized bytes until the result is released.
        ///
        /// # Safety
        /// A non-null result must be live throughout this call and any use of its bytes.
        pub unsafe fn $pointer(handle: *const OperationResult) -> *const u8 {
            unsafe { handle.as_ref() }.map_or(std::ptr::null(), |value| value.$field.as_ptr())
        }
        /// Returns the corresponding byte length.
        ///
        /// # Safety
        /// A non-null result must be live throughout this call.
        pub unsafe fn $length(handle: *const OperationResult) -> usize {
            unsafe { handle.as_ref() }.map_or(0, |value| value.$field.len())
        }
    };
}
bytes!(result_json, result_json_length, json);
bytes!(result_error, result_error_length, error);

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
mod tests {
    use super::*;

    #[test]
    fn contains_recoverable_panics() {
        let error = contain::<()>(|| panic!("injected panic")).unwrap_err();
        assert_eq!(error.category, ErrorCategory::Internal);
    }

    #[test]
    fn body_storage_is_opt_in_and_range_errors_match_the_application() {
        unsafe {
            let bad_path = application_create_with_body_store(b"\xff".as_ptr(), 1);
            assert_eq!(application_valid(bad_path), 0);
            let failure = status(bad_path);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&(*failure).error).unwrap()["category"],
                "invalid-input"
            );
            release(failure);
            application_release(bad_path);
            let path = std::env::temp_dir().join(format!(
                "transmog-sdk-body-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let bytes = path.to_str().unwrap().as_bytes();
            let app = application_create_with_body_store(bytes.as_ptr(), bytes.len());
            assert_eq!(application_valid(app), 1);
            let state = state(app).unwrap();
            assert!(state.application.body_store().is_some());
            let request = br#"{"sessionId":"00000000000000000000000000000000","boundary":"client-response","offset":"0","maxBytes":0}"#;
            let expected = state
                .runtime
                .block_on(
                    state
                        .application
                        .read_body_range(serde_json::from_slice(request).unwrap()),
                )
                .unwrap_err();
            let actual = read_body_range(app, request.as_ptr(), request.len());
            assert_eq!((*actual).error, serde_json::to_vec(&expected).unwrap());
            release(actual);
            application_release(app);
            assert!(
                !path.exists(),
                "The default memory body store should not create files"
            );
        }
    }

    #[test]
    fn traffic_view_projection_preserves_shared_validation_and_release() {
        let app = application_create();
        unsafe {
            let view = query_traffic_view(app, b"{}".as_ptr(), 2);
            let value: serde_json::Value = serde_json::from_slice(&(*view).json).unwrap();
            let id = value["id"].as_str().unwrap();
            let expected = state(app)
                .unwrap()
                .application
                .traffic_view_rows(id, 0, 1)
                .unwrap();
            let rows = traffic_view_rows(app, id.as_ptr(), id.len(), 0, 1);
            assert_eq!((*rows).json, json(expected).unwrap());
            release(rows);
            let expected = state(app)
                .unwrap()
                .application
                .traffic_view_rows(id, 0, 0)
                .unwrap_err();
            let invalid = traffic_view_rows(app, id.as_ptr(), id.len(), 0, 0);
            assert_eq!((*invalid).error, serde_json::to_vec(&expected).unwrap());
            release(invalid);
            let released = release_traffic_view(app, id.as_ptr(), id.len());
            assert_eq!((*released).json, b"null");
            release(released);
            let expired = traffic_view_rows(app, id.as_ptr(), id.len(), 0, 1);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&(*expired).error).unwrap()["category"],
                "unavailable"
            );
            release(expired);
            release(view);
            application_release(app);
        }
    }

    #[test]
    fn subscriptions_deliver_shared_hints_and_can_outlive_the_application_owner() {
        unsafe extern "C" fn receive(context: usize, bytes: *const u8, length: usize) {
            let sender = unsafe { &*(context as *const std::sync::mpsc::Sender<Vec<u8>>) };
            let _ = sender.send(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec());
        }
        let app = application_create();
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
        unsafe {
            let invalid = subscribe_session_updates(app, 0, 0);
            assert_eq!(session_updates_valid(invalid), 0);
            let status = session_updates_status(invalid);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&(*status).error).unwrap()["category"],
                "invalid-input"
            );
            release(status);
            session_updates_release(invalid);
            let updates = subscribe_session_updates(
                app,
                receive as *const () as usize,
                (&raw const sender) as usize,
            );
            assert_eq!(session_updates_valid(updates), 1);
            let state = state(app).unwrap();
            state
                .application
                .save_product_state(state.application.product_state())
                .unwrap();
            let hint: serde_json::Value = serde_json::from_slice(
                &receiver
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap(),
            )
            .unwrap();
            assert!(hint["sequence"].is_u64());
            assert!(hint["lagged"].is_boolean());
            application_release(app);
            session_updates_release(updates);
            session_updates_release(std::ptr::null_mut());
        }
    }

    #[test]
    fn projects_application_models_and_validation_without_another_domain_model() {
        let app = application_create();
        unsafe {
            assert_eq!(application_valid(app), 1);
            let state = state(app).unwrap();
            let status = status(app);
            assert_eq!((*status).json, json(state.application.status()).unwrap());
            release(status);
            let request = br#"{"limit":0}"#;
            let expected = state
                .application
                .query_sessions(serde_json::from_slice(request).unwrap())
                .unwrap_err();
            let actual = query_sessions(app, request.as_ptr(), request.len());
            assert_eq!((*actual).error, serde_json::to_vec(&expected).unwrap());
            release(actual);
            let too_large = query_sessions(app, std::ptr::null(), MAX_INPUT_BYTES + 1);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&(*too_large).error).unwrap()["category"],
                "limit"
            );
            release(too_large);
            application_release(app);
        }
    }
}
