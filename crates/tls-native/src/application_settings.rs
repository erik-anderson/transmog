//! Borrowed, synchronous ALPS operations on the shared `BoringSSL` instance.
use boring::{error::ErrorStack, ssl::SslRef};
use foreign_types::ForeignTypeRef;

/// Offers HTTP/2 ALPS at Chromium's codepoint, with the supplied local settings.
/// An empty client value matches Chromium; HTTP/2 still sends its local SETTINGS.
/// # Errors
/// Returns the native configuration error.
pub fn configure_h2_application_settings(
    ssl: &mut SslRef,
    settings: &[u8],
) -> Result<(), ErrorStack> {
    // SAFETY: SSL is exclusively borrowed and live; BoringSSL copies both slices
    // synchronously. The codepoint setter retains no borrowed data.
    let result = unsafe {
        boring_sys::SSL_set_alps_use_new_codepoint(ssl.as_ptr(), 1);
        boring_sys::SSL_add_application_settings(
            ssl.as_ptr(),
            b"h2".as_ptr(),
            2,
            settings.as_ptr(),
            settings.len(),
        )
    };
    if result == 1 {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

/// Negotiated peer ALPS bytes, borrowed only while SSL remains unchanged.
/// `Some(&[])` distinguishes an empty negotiated value from no negotiation.
pub fn peer_application_settings(ssl: &SslRef) -> Option<&[u8]> {
    // SAFETY: SSL is live. The getter returns storage owned by this SSL object
    // and guaranteed valid until its next mutation, which the borrow prevents.
    unsafe {
        if boring_sys::SSL_has_application_settings(ssl.as_ptr()) == 0 {
            return None;
        }
        let mut data = std::ptr::null();
        let mut length = 0;
        boring_sys::SSL_get0_peer_application_settings(
            ssl.as_ptr(),
            &raw mut data,
            &raw mut length,
        );
        if length == 0 {
            Some(&[])
        } else {
            Some(std::slice::from_raw_parts(data, length))
        }
    }
}
