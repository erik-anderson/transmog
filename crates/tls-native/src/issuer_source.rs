//! Owned issuer requests; fetched and cached candidates never confer trust.
use std::{
    ffi::c_void,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
/// One bounded batch of issuer URLs, whose destruction cancels remaining work.
pub trait IssuerRequest: Send {
    /// Returns the next URL result in order, or None when exhausted.
    /// An empty result represents an unsuccessful or unusable response.
    fn next(&mut self) -> Option<Vec<u8>>;
}
/// Network and cache inputs used by the one native PKI path builder.
pub trait IssuerSource: fmt::Debug + Send + Sync {
    /// Starts at most five URLs with a common deadline and cancellation signal.
    fn start(
        &self,
        urls: &[String],
        timeout: Duration,
        cancelled: Arc<AtomicBool>,
    ) -> Box<dyn IssuerRequest>;
    /// Returns bounded untrusted intermediate candidates.
    fn cached_issuers(&self) -> Vec<Vec<u8>> {
        Vec::new()
    }
    /// Retains only intermediate certificates from a completely accepted path.
    fn cache_verified_path(&self, _path: &[Vec<u8>]) {}
}

/// Borrowed callback table: all requests are destroyed before verification returns.
#[repr(C)]
pub(crate) struct Hooks {
    context: *mut c_void,
    valid_url: unsafe extern "C" fn(*const u8, usize) -> bool,
    start: unsafe extern "C" fn(
        *mut c_void,
        *const *const u8,
        *const usize,
        usize,
        u64,
    ) -> *mut c_void,
    next: unsafe extern "C" fn(
        *mut c_void,
        unsafe extern "C" fn(*mut c_void, *const u8, usize),
        *mut c_void,
    ) -> bool,
    destroy: unsafe extern "C" fn(*mut c_void),
    cancelled: unsafe extern "C" fn(*mut c_void) -> bool,
}
pub(crate) struct FetchContext {
    pub source: Option<Arc<dyn IssuerSource>>,
    pub cancelled: Arc<AtomicBool>,
    pub attempts: usize,
}
impl FetchContext {
    pub fn hooks(&mut self) -> Hooks {
        Hooks {
            context: std::ptr::from_mut(self).cast(),
            valid_url,
            start,
            next,
            destroy,
            cancelled,
        }
    }
}
unsafe extern "C" fn valid_url(bytes: *const u8, len: usize) -> bool {
    std::panic::catch_unwind(|| {
        // SAFETY: PKI borrows the AIA URI for this call only.
        let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
        std::str::from_utf8(bytes)
            .ok()
            .is_some_and(|url| url::Url::parse(url).is_ok())
    })
    .unwrap_or(false)
}
struct Request(Box<dyn IssuerRequest>);
unsafe extern "C" fn start(
    context: *mut c_void,
    urls: *const *const u8,
    lengths: *const usize,
    count: usize,
    timeout_ms: u64,
) -> *mut c_void {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: synchronous PKI calls borrow the stack context and at most five URIs.
        let context = unsafe { &mut *context.cast::<FetchContext>() };
        let urls = unsafe { std::slice::from_raw_parts(urls, count) };
        let lengths = unsafe { std::slice::from_raw_parts(lengths, count) };
        let urls: Vec<_> = urls
            .iter()
            .zip(lengths)
            .filter_map(|(ptr, len)| {
                let bytes = unsafe { std::slice::from_raw_parts(*ptr, *len) };
                std::str::from_utf8(bytes).ok().map(str::to_owned)
            })
            .collect();
        let Some(source) = &context.source else {
            return std::ptr::null_mut();
        };
        context.attempts += urls.len();
        let request = source.start(
            &urls,
            Duration::from_millis(timeout_ms),
            context.cancelled.clone(),
        );
        Box::into_raw(Box::new(Request(request))).cast()
    }))
    .unwrap_or(std::ptr::null_mut())
}
unsafe extern "C" fn next(
    request: *mut c_void,
    accept: unsafe extern "C" fn(*mut c_void, *const u8, usize),
    output: *mut c_void,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: exclusively owned native Request calls; response is borrowed for parsing only.
        let request = unsafe { &mut *request.cast::<Request>() };
        let Some(bytes) = request.0.next() else {
            return false;
        };
        if bytes.len() <= 65536 {
            unsafe { accept(output, bytes.as_ptr(), bytes.len()) };
        }
        true
    }))
    .unwrap_or(false)
}
unsafe extern "C" fn destroy(request: *mut c_void) {
    // Destructors supplied by embedders cannot unwind across the C++ bridge.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: unique pointer created by start; native request destroys it exactly once.
        drop(unsafe { Box::from_raw(request.cast::<Request>()) });
    }));
}
unsafe extern "C" fn cancelled(context: *mut c_void) -> bool {
    // SAFETY: context outlives the native path builder and its requests.
    unsafe { &*context.cast::<FetchContext>() }
        .cancelled
        .load(Ordering::Acquire)
}
