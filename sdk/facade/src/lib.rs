//! Private pointer-only Crubit surface. All domain behavior lives in transmog-app.
#![deny(unsafe_op_in_unsafe_fn)]
use transmog_sdk_engine as engine;

/// Creates an owned application handle.
pub fn application_create() -> *mut u8 {
    engine::application_create().cast()
}

/// Creates an application with the existing product body store.
///
/// # Safety
/// The `length` UTF-8 cache-path bytes must remain readable throughout the call.
pub unsafe fn application_create_with_body_store(input: *const u8, length: usize) -> *mut u8 {
    unsafe { engine::application_create_with_body_store(input, length).cast() }
}

macro_rules! handle_call {
    ($name:ident, $result:ty) => {
        /// Projects the corresponding engine ownership adapter.
        ///
        /// # Safety
        /// The handle must be live and not concurrently released; release consumes it.
        pub unsafe fn $name(handle: *mut u8) -> $result {
            unsafe { engine::$name(handle.cast()) }
        }
    };
}
handle_call!(application_valid, i32);
handle_call!(application_release, ());
handle_call!(release, ());
handle_call!(succeeded, i32);
handle_call!(result_json, *const u8);
handle_call!(result_json_length, usize);
handle_call!(result_error, *const u8);
handle_call!(result_error_length, usize);
handle_call!(session_updates_valid, i32);
handle_call!(session_updates_release, ());
handle_call!(body_page_succeeded, i32);
handle_call!(body_page_release, ());

macro_rules! operation {
    ($name:ident) => {
        /// Executes the matching application operation.
        ///
        /// # Safety
        /// The application must be live throughout the call, without concurrent release.
        pub unsafe fn $name(handle: *mut u8) -> *mut u8 {
            unsafe { engine::$name(handle.cast()).cast() }
        }
    };
}
operation!(status);
operation!(product_state);
operation!(stop_proxy);
operation!(shutdown);
operation!(session_updates_status);

macro_rules! input_operation {
    ($name:ident) => {
        /// Executes the matching application operation using its existing schema.
        ///
        /// # Safety
        /// The application and the `length` readable input bytes must remain live.
        pub unsafe fn $name(handle: *mut u8, input: *const u8, length: usize) -> *mut u8 {
            unsafe { engine::$name(handle.cast(), input, length).cast() }
        }
    };
}
input_operation!(save_product_state);
input_operation!(start_proxy);
input_operation!(query_sessions);
input_operation!(session_detail);
input_operation!(query_traffic_view);
input_operation!(release_traffic_view);

/// Reads an owned typed page from the shared Application operation.
///
/// # Safety
/// Application and UTF-8 session/boundary inputs must remain live for this call.
pub unsafe fn read_body_range(
    handle: *mut u8,
    session: *const u8,
    session_length: usize,
    boundary: *const u8,
    boundary_length: usize,
    offset: u64,
    max_bytes: usize,
) -> *mut u8 {
    unsafe {
        engine::read_body_range(
            handle.cast(),
            session,
            session_length,
            boundary,
            boundary_length,
            offset,
            max_bytes,
        )
        .cast()
    }
}

macro_rules! body_field {
    ($name:ident, $field:ty, $result:ty) => {
        /// Borrows a primitive field from an owned body page.
        ///
        /// # Safety
        /// Keep the page live during the call and any use of borrowed bytes.
        pub unsafe fn $name(handle: *mut u8, field: $field) -> $result {
            unsafe { engine::$name(handle.cast(), field) }
        }
    };
}
body_field!(body_page_number, u32, u64);
body_field!(body_page_data, u32, *const u8);
body_field!(body_page_data_length, u32, usize);
body_field!(body_page_coding, usize, *const u8);
body_field!(body_page_coding_length, usize, usize);

/// Reads the shared traffic view's bounded row window.
///
/// # Safety
/// The application and readable ID bytes must remain live throughout the call.
pub unsafe fn traffic_view_rows(
    handle: *mut u8,
    input: *const u8,
    length: usize,
    offset: usize,
    limit: usize,
) -> *mut u8 {
    unsafe { engine::traffic_view_rows(handle.cast(), input, length, offset, limit).cast() }
}

/// Creates a callback subscription; addresses stay private to the native adapter.
///
/// # Safety
/// The application must be live, and callback/context must satisfy the engine contract.
pub unsafe fn subscribe_session_updates(
    handle: *mut u8,
    callback: usize,
    context: usize,
) -> *mut u8 {
    unsafe { engine::subscribe_session_updates(handle.cast(), callback, context).cast() }
}
