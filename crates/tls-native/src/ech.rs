//! ECH retry ownership, captured before QUIC drains the native error queue.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{Ssl, SslRef},
};
use foreign_types::ForeignType;
use std::sync::{Arc, Mutex, OnceLock};
type State = Arc<Mutex<Option<Vec<u8>>>>;
static INDEX: OnceLock<Result<Index<Ssl, State>, ErrorStack>> = OnceLock::new();
/// Installs a real `ECHConfigList` before the first `ClientHello`.
/// # Errors
/// Returns invalid configuration or native allocation errors.
pub fn configure_ech(ssl: &mut SslRef, config: &[u8]) -> Result<(), ErrorStack> {
    ssl.set_ech_config_list(config)?;
    ssl.set_ex_data(
        INDEX.get_or_init(Ssl::new_ex_index::<State>).clone()?,
        Arc::default(),
    );
    Ok(())
}
/// Whether a real configuration was installed on this SSL.
pub fn ech_configured(ssl: &SslRef) -> bool {
    INDEX
        .get()
        .and_then(|i| i.as_ref().ok())
        .is_some_and(|i| ssl.ex_data(*i).is_some())
}
/// Copies authenticated retry parameters observed at the handshake exit.
pub fn authenticated_ech_retry(ssl: &SslRef) -> Option<Vec<u8>> {
    INDEX
        .get()
        .and_then(|i| i.as_ref().ok())
        .and_then(|i| ssl.ex_data(*i))?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}
/// Records ECH rejection only at `CONNECT_EXIT`, while the specific native error
/// is still present. Other failures must never access the retry-config getter.
pub fn record_ech_exit(ssl: &SslRef) {
    let Some(state) = INDEX
        .get()
        .and_then(|i| i.as_ref().ok())
        .and_then(|i| ssl.ex_data(*i))
    else {
        return;
    };
    // SAFETY: synchronous read of this thread's native error queue.
    let error = unsafe { boring_sys::ERR_peek_last_error() };
    // Bindgen represents this C enum's backing integer differently on Windows
    // and Linux. Widen both nonnegative library IDs without a signed cast.
    if i64::from((error >> 24) & 0xff) != i64::from(boring_sys::ERR_LIB_SSL.0)
        || error & 0xfff != boring_sys::SSL_R_ECH_REJECTED.unsigned_abs()
    {
        return;
    }
    let Some(name) = ssl
        .get_ech_name_override()
        .and_then(|name| std::str::from_utf8(name).ok())
    else {
        return;
    };
    if !crate::peer_verified_for_name(ssl, name) {
        return;
    }
    // The library authenticates Finished before emitting ECH_REJECTED. Only
    // this exact failure satisfies the retry getter's assertion contract.
    let config = ssl.get_ech_retry_configs().unwrap_or_default();
    if config.len() <= 65535 {
        *state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config.to_vec());
    }
}
/// Serializes a public server ECH config using the native library's format.
/// # Errors
/// Returns invalid public-name or native serialization errors.
pub fn marshal_ech_config(
    key: &boring::hpke::HpkeKey,
    public_name: &str,
    id: u8,
) -> Result<Vec<u8>, ErrorStack> {
    let name = std::ffi::CString::new(public_name).map_err(|_| ErrorStack::get())?;
    let mut bytes = std::ptr::null_mut();
    let mut length = 0;
    // SAFETY: borrowed key/name are live; successful output is an owned native buffer.
    if unsafe {
        boring_sys::SSL_marshal_ech_config(
            &raw mut bytes,
            &raw mut length,
            id,
            key.as_ptr(),
            name.as_ptr(),
            255,
        )
    } != 1
    {
        return Err(ErrorStack::get());
    }
    let config = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
    unsafe { boring_sys::OPENSSL_free(bytes.cast()) };
    Ok(config)
}
