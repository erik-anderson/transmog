//! Owned typed body results. No body JSON or base64 crosses the native boundary.
use super::{ApplicationHandle, MAX_INPUT_BYTES, contain, error, input, state};
use transmog_app::{AppError, BodyBytePage, BodyRangeRequest, ErrorCategory};

/// Owns one shared Application byte page or its original structured error.
pub struct BodyPageHandle {
    pub(crate) value: Result<BodyBytePage, AppError>,
}

/// Reads a bounded body page with numeric offsets and the existing validation.
///
/// # Safety
/// The application and both UTF-8 input slices must remain live for the call.
pub unsafe fn read_body_range(
    handle: *const ApplicationHandle,
    session: *const u8,
    session_length: usize,
    boundary: *const u8,
    boundary_length: usize,
    offset: u64,
    max_bytes: usize,
) -> *mut BodyPageHandle {
    let value = contain(|| {
        let state = unsafe { state(handle) }?;
        if session_length.saturating_add(boundary_length) > MAX_INPUT_BYTES {
            return Err(error(
                ErrorCategory::Limit,
                "SDK input exceeds 262144 bytes",
            ));
        }
        let session = std::str::from_utf8(unsafe { input(session, session_length) }?)
            .map_err(|_| error(ErrorCategory::InvalidInput, "session ID must be UTF-8"))?;
        let boundary = std::str::from_utf8(unsafe { input(boundary, boundary_length) }?)
            .map_err(|_| error(ErrorCategory::InvalidInput, "body boundary must be UTF-8"))?;
        // BodyRangeRequest retains its desktop input schema and error behavior.
        // Only that small input adapter formats text; no JSON is produced here.
        state
            .runtime
            .block_on(state.application.read_body_range(BodyRangeRequest {
                session_id: session.into(),
                boundary: boundary.into(),
                offset: offset.to_string(),
                max_bytes: Some(max_bytes),
            }))
    });
    Box::into_raw(Box::new(BodyPageHandle { value }))
}

/// Reports operation success; null is an empty/failed owner.
///
/// # Safety
/// A non-null owner must remain live for this call.
pub unsafe fn body_page_succeeded(handle: *const BodyPageHandle) -> i32 {
    unsafe { handle.as_ref() }
        .is_some_and(|page| page.value.is_ok())
        .into()
}

/// Borrows primitive fields for the private C++ projection. Field IDs are private
/// to this adapter and sdk.cpp; no Rust layout is exported.
///
/// # Safety
/// A non-null owner must remain live for this call.
pub unsafe fn body_page_number(handle: *const BodyPageHandle, field: u32) -> u64 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return 0;
    };
    match &handle.value {
        Ok(page) => match field {
            0 => page.offset,
            1 => page.total_bytes,
            2 => page.read_limit as u64,
            3 => u64::from(super::models::body_availability_number(
                &page.metadata.availability,
            )),
            4 => u64::from(page.metadata.length_known),
            5 => page.metadata.observed_bytes,
            6 => page.metadata.retained_bytes,
            7 => u64::from(page.metadata.wire_body_bytes.is_some()),
            8 => page.metadata.wire_body_bytes.unwrap_or(0),
            9 => page.metadata.content_codings.len() as u64,
            _ => 0,
        },
        Err(error) => match field {
            10 => u64::from(super::models::error_category_number(&error.category)),
            11 => u64::from(error.retryable),
            _ => 0,
        },
    }
}

unsafe fn data<'a>(handle: *const BodyPageHandle, field: u32) -> Option<&'a [u8]> {
    match &(unsafe { handle.as_ref() })?.value {
        Ok(page) => match field {
            0 => Some(page.bytes.as_slice()),
            1 => Some(page.metadata.exchange_id.as_bytes()),
            2 => Some(page.metadata.boundary.as_bytes()),
            3 => page.metadata.media_type.as_deref().map(str::as_bytes),
            4 => page.metadata.charset.as_deref().map(str::as_bytes),
            5 => page.metadata.sha256.as_deref().map(str::as_bytes),
            6 => page.metadata.reason.as_deref().map(str::as_bytes),
            _ => None,
        },
        Err(error) if field == 7 => Some(error.message.as_bytes()),
        Err(_) => None,
    }
}

/// Borrows raw data or UTF-8 metadata until the owner is released.
///
/// # Safety
/// Keep a non-null owner live throughout this call and use of the returned bytes.
pub unsafe fn body_page_data(handle: *const BodyPageHandle, field: u32) -> *const u8 {
    unsafe { data(handle, field) }.map_or(std::ptr::null(), <[u8]>::as_ptr)
}

/// Returns the matching borrowed data length.
///
/// # Safety
/// A non-null owner must remain live for this call.
pub unsafe fn body_page_data_length(handle: *const BodyPageHandle, field: u32) -> usize {
    unsafe { data(handle, field) }.map_or(0, <[u8]>::len)
}

unsafe fn coding<'a>(handle: *const BodyPageHandle, index: usize) -> Option<&'a str> {
    unsafe { handle.as_ref() }?
        .value
        .as_ref()
        .ok()?
        .metadata
        .content_codings
        .get(index)
        .map(String::as_str)
}

/// Borrows one content-coding token; an out-of-range index returns null.
///
/// # Safety
/// Keep a non-null owner live throughout this call and use of the returned bytes.
pub unsafe fn body_page_coding(handle: *const BodyPageHandle, index: usize) -> *const u8 {
    unsafe { coding(handle, index) }.map_or(std::ptr::null(), str::as_ptr)
}

/// Returns the matching content-coding token's byte length.
///
/// # Safety
/// A non-null owner must remain live for this call.
pub unsafe fn body_page_coding_length(handle: *const BodyPageHandle, index: usize) -> usize {
    unsafe { coding(handle, index) }.map_or(0, str::len)
}

/// Releases one owner exactly once; null is accepted.
///
/// # Safety
/// A non-null owner must be exclusive, with no borrowed views still in use.
pub unsafe fn body_page_release(handle: *mut BodyPageHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_app::{BodyAvailability, StoredBodyMetadata};

    #[test]
    fn primitive_projection_preserves_exact_bytes_metadata_and_structured_errors() {
        let length = (1_u64 << 53) + 19;
        let metadata = StoredBodyMetadata {
            length_known: false,
            exchange_id: "session".into(),
            boundary: "client-response",
            observed_bytes: length + 1,
            wire_body_bytes: None,
            retained_bytes: length,
            availability: BodyAvailability::Capturing,
            media_type: Some("application/octet-stream".into()),
            charset: Some(String::new()),
            content_codings: vec!["gzip".into(), "br".into()],
            sha256: None,
            reason: Some("reason".into()),
        };
        let handle = Box::into_raw(Box::new(BodyPageHandle {
            value: Ok(BodyBytePage {
                metadata,
                total_bytes: length,
                offset: length - 3,
                bytes: vec![0, 255, 1],
                read_limit: 65536,
            }),
        }));
        unsafe {
            assert_eq!(body_page_succeeded(handle), 1);
            let numbers = [
                length - 3,
                length,
                65536,
                1,
                0,
                length + 1,
                length,
                0,
                0,
                2,
                0,
                0,
            ];
            for (field, expected) in numbers.into_iter().enumerate() {
                assert_eq!(body_page_number(handle, field as u32), expected);
            }
            let fields: [Option<&[u8]>; 8] = [
                Some(&[0, 255, 1]),
                Some(b"session"),
                Some(b"client-response"),
                Some(b"application/octet-stream"),
                Some(b""),
                None,
                Some(b"reason"),
                None,
            ];
            for (field, expected) in fields.into_iter().enumerate() {
                let pointer = body_page_data(handle, field as u32);
                let size = body_page_data_length(handle, field as u32);
                match expected {
                    Some(bytes) => {
                        assert!(!pointer.is_null());
                        assert_eq!(std::slice::from_raw_parts(pointer, size), bytes);
                    }
                    None => {
                        assert!(pointer.is_null());
                        assert_eq!(size, 0);
                    }
                }
            }
            assert_eq!(
                std::slice::from_raw_parts(
                    body_page_coding(handle, 0),
                    body_page_coding_length(handle, 0)
                ),
                b"gzip"
            );
            assert_eq!(
                std::slice::from_raw_parts(
                    body_page_coding(handle, 1),
                    body_page_coding_length(handle, 1)
                ),
                b"br"
            );
            assert!(body_page_coding(handle, 2).is_null());
            body_page_release(handle);
        }
        for (category, number) in [
            (ErrorCategory::PasswordRequired, 1),
            (ErrorCategory::InvalidPassword, 2),
            (ErrorCategory::InvalidInput, 3),
            (ErrorCategory::Unsupported, 4),
            (ErrorCategory::Canceled, 5),
            (ErrorCategory::Conflict, 6),
            (ErrorCategory::Limit, 7),
            (ErrorCategory::Unavailable, 8),
            (ErrorCategory::Retryable, 9),
            (ErrorCategory::Internal, 10),
        ] {
            let handle = Box::into_raw(Box::new(BodyPageHandle {
                value: Err(AppError {
                    category,
                    message: "original message".into(),
                    retryable: true,
                }),
            }));
            unsafe {
                assert_eq!(body_page_succeeded(handle), 0);
                assert_eq!(body_page_number(handle, 10), number);
                assert_eq!(body_page_number(handle, 11), 1);
                assert_eq!(
                    std::slice::from_raw_parts(
                        body_page_data(handle, 7),
                        body_page_data_length(handle, 7)
                    ),
                    b"original message"
                );
                assert!(body_page_data(handle, 0).is_null());
                body_page_release(handle);
            }
        }
    }
}
