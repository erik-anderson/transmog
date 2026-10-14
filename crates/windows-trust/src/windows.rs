use super::{LocalCertificates, LocalTrustError, State};
use std::{
    mem::size_of,
    ptr,
    sync::{Arc, atomic::Ordering},
};
use windows_sys::Win32::{
    Foundation::{
        CRYPT_E_NOT_FOUND, CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GetLastError,
        HANDLE, SetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::Cryptography::{
        CERT_CONTEXT, CERT_STORE_CTRL_NOTIFY_CHANGE, CERT_STORE_CTRL_RESYNC,
        CERT_STORE_OPEN_EXISTING_FLAG, CERT_STORE_PROV_COLLECTION,
        CERT_STORE_PROV_SYSTEM_REGISTRY_W, CERT_STORE_READONLY_FLAG,
        CERT_SYSTEM_STORE_CURRENT_USER, CERT_SYSTEM_STORE_CURRENT_USER_GROUP_POLICY_ID,
        CERT_SYSTEM_STORE_LOCAL_MACHINE, CERT_SYSTEM_STORE_LOCAL_MACHINE_ENTERPRISE_ID,
        CERT_SYSTEM_STORE_LOCAL_MACHINE_GROUP_POLICY_ID, CERT_SYSTEM_STORE_LOCATION_SHIFT,
        CTL_USAGE, CertAddStoreToCollection, CertCloseStore, CertControlStore,
        CertEnumCertificatesInStore, CertFreeCertificateContext, CertGetEnhancedKeyUsage,
        CertOpenStore, HCERTSTORE,
    },
    System::Threading::{
        CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
    },
};
const MAX_CERTS: usize = 8192;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_CERT_BYTES: usize = 1024 * 1024;
const MAX_PROPERTY_BYTES: usize = 64 * 1024;
const NOT_FOUND: u32 = u32::from_ne_bytes(CRYPT_E_NOT_FOUND.to_ne_bytes());
const MACHINE_POLICY: u32 =
    CERT_SYSTEM_STORE_LOCAL_MACHINE_GROUP_POLICY_ID << CERT_SYSTEM_STORE_LOCATION_SHIFT;
const MACHINE_ENTERPRISE: u32 =
    CERT_SYSTEM_STORE_LOCAL_MACHINE_ENTERPRISE_ID << CERT_SYSTEM_STORE_LOCATION_SHIFT;
const USER_POLICY: u32 =
    CERT_SYSTEM_STORE_CURRENT_USER_GROUP_POLICY_ID << CERT_SYSTEM_STORE_LOCATION_SHIFT;

fn error(operation: &'static str) -> LocalTrustError {
    // SAFETY: reading the calling thread's error immediately after a failed API.
    LocalTrustError::Windows {
        operation,
        code: unsafe { GetLastError() },
    }
}
pub(super) struct Event(HANDLE);
// SAFETY: Windows event handles support concurrent signal/wait operations; Arc
// keeps their owned handle alive until every caller and the worker has released it.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}
impl Event {
    fn new() -> Result<Self, LocalTrustError> {
        // SAFETY: no borrowed security/name arguments; the returned event is owned.
        let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if handle.is_null() {
            Err(error("event creation"))
        } else {
            Ok(Self(handle))
        }
    }
    pub(super) fn is_signaled(&self) -> bool {
        // SAFETY: live event, zero timeout, no external memory or filesystem IO.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
    fn signal(&self) {
        // SAFETY: the owned event stays live through this operation.
        unsafe {
            SetEvent(self.0);
        }
    }
    fn reset(&self) -> Result<(), LocalTrustError> {
        // SAFETY: live owned manual-reset event, called by its sole worker.
        if unsafe { ResetEvent(self.0) } == 0 {
            Err(error("event reset"))
        } else {
            Ok(())
        }
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: final owner releases the handle exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct Control {
    pub(super) changed: Arc<Event>,
    stop: Arc<Event>,
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stop.signal();
    }
}
struct Store(HCERTSTORE);
impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the store reference returned by Windows.
        unsafe {
            CertCloseStore(self.0, 0);
        }
    }
}
impl Store {
    fn collection() -> Result<Self, LocalTrustError> {
        // SAFETY: collection provider takes no extra parameter and returns ownership.
        let handle = unsafe { CertOpenStore(CERT_STORE_PROV_COLLECTION, 0, 0, 0, ptr::null()) };
        if handle.is_null() {
            Err(error("collection open"))
        } else {
            Ok(Self(handle))
        }
    }
    fn add_registry(&self, location: u32, name: &str) -> Result<(), LocalTrustError> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: registry provider with a terminated name valid through the call;
        // read-only OPEN_EXISTING excludes aggregate AuthRoot/public-root providers.
        let handle = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_REGISTRY_W,
                0,
                0,
                location | CERT_STORE_OPEN_EXISTING_FLAG | CERT_STORE_READONLY_FLAG,
                name.as_ptr().cast(),
            )
        };
        if handle.is_null() {
            let failure = error("registry store open");
            return match failure {
                LocalTrustError::Windows {
                    code: ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND,
                    ..
                } => Ok(()),
                other => Err(other),
            };
        }
        let child = Store(handle);
        // SAFETY: collection takes a reference to a live child; it is read-only.
        if unsafe { CertAddStoreToCollection(self.0, child.0, 0, 0) } == 0 {
            return Err(error("collection add"));
        }
        Ok(())
    }
    fn control(&self, action: u32, event: &Event) -> Result<(), LocalTrustError> {
        let mut handle = event.0;
        // SAFETY: live store and event, pointer is valid throughout the call.
        // Notify/resync copy the event handle and retain their own references.
        if unsafe { CertControlStore(self.0, 0, action, (&raw mut handle).cast()) } == 0 {
            Err(error("notification/resynchronization"))
        } else {
            Ok(())
        }
    }
}
struct Collections {
    roots: Store,
    intermediates: Store,
    leaves: Store,
    blocked: Store,
}
impl Collections {
    fn open() -> Result<Self, LocalTrustError> {
        let result = Self {
            roots: Store::collection()?,
            intermediates: Store::collection()?,
            leaves: Store::collection()?,
            blocked: Store::collection()?,
        };
        for location in [
            CERT_SYSTEM_STORE_LOCAL_MACHINE,
            MACHINE_POLICY,
            MACHINE_ENTERPRISE,
            CERT_SYSTEM_STORE_CURRENT_USER,
            USER_POLICY,
        ] {
            result.roots.add_registry(location, "ROOT")?;
            result.intermediates.add_registry(location, "CA")?;
            result.blocked.add_registry(location, "Disallowed")?;
            if location != CERT_SYSTEM_STORE_CURRENT_USER && location != USER_POLICY {
                result.leaves.add_registry(location, "TrustedPeople")?;
            }
        }
        Ok(result)
    }
    fn control(&self, action: u32, event: &Event) -> Result<(), LocalTrustError> {
        for store in [
            &self.roots,
            &self.intermediates,
            &self.leaves,
            &self.blocked,
        ] {
            store.control(action, event)?;
        }
        Ok(())
    }
    fn read(&self, generation: u64) -> Result<LocalCertificates, LocalTrustError> {
        let mut result = LocalCertificates {
            generation,
            ..Default::default()
        };
        let mut budget = Budget::default();
        for (store, output, check_usage) in [
            (&self.roots, &mut result.anchors, true),
            (&self.intermediates, &mut result.intermediates, false),
            (&self.leaves, &mut result.trusted_leaves, true),
            (&self.blocked, &mut result.disallowed, false),
        ] {
            enumerate(store, output, check_usage, &mut budget)?;
        }
        Ok(result.canonical())
    }
}
#[derive(Default)]
struct Budget {
    count: usize,
    bytes: usize,
}
struct Certificate(*const CERT_CONTEXT);
impl Drop for Certificate {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the non-null enumerated context reference is owned here.
            unsafe {
                CertFreeCertificateContext(self.0);
            }
        }
    }
}
fn enumerate(
    store: &Store,
    out: &mut Vec<Vec<u8>>,
    usage: bool,
    budget: &mut Budget,
) -> Result<(), LocalTrustError> {
    let mut cert = Certificate(ptr::null());
    loop {
        let previous = std::mem::replace(&mut cert.0, ptr::null());
        // SAFETY: enumeration consumes the previous owned context, even on failure.
        cert.0 = unsafe { CertEnumCertificatesInStore(store.0, previous) };
        if cert.0.is_null() {
            let end = error("enumeration");
            return if matches!(
                end,
                LocalTrustError::Windows {
                    code: NOT_FOUND,
                    ..
                }
            ) {
                Ok(())
            } else {
                Err(end)
            };
        }
        budget.count += 1;
        // SAFETY: the enumerated owned context remains live throughout this iteration.
        let context = unsafe { &*cert.0 };
        let length = context.cbCertEncoded as usize;
        budget.bytes = budget
            .bytes
            .checked_add(length)
            .ok_or(LocalTrustError::Limit)?;
        if budget.count > MAX_CERTS
            || length == 0
            || length > MAX_CERT_BYTES
            || budget.bytes > MAX_BYTES
            || context.pbCertEncoded.is_null()
        {
            return Err(LocalTrustError::Limit);
        }
        if !usage || server_auth(cert.0).unwrap_or(false) {
            // SAFETY: CERT_CONTEXT owns this bounded public DER allocation.
            out.push(unsafe { std::slice::from_raw_parts(context.pbCertEncoded, length) }.to_vec());
        }
    }
}
fn server_auth(cert: *const CERT_CONTEXT) -> Result<bool, LocalTrustError> {
    let mut bytes = 0;
    // SAFETY: live context, size query returns no borrowed data.
    if unsafe { CertGetEnhancedKeyUsage(cert, 0, ptr::null_mut(), &raw mut bytes) } == 0 {
        return Err(error("usage properties"));
    }
    let bytes = bytes as usize;
    if !(size_of::<CTL_USAGE>()..=MAX_PROPERTY_BYTES).contains(&bytes) {
        return Err(LocalTrustError::Limit);
    }
    let mut buffer = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
    let usage = buffer.as_mut_ptr().cast::<CTL_USAGE>();
    let mut length = u32::try_from(bytes).map_err(|_| LocalTrustError::Limit)?;
    // SAFETY: usize-aligned buffer covers the queried native structure and pointers.
    unsafe {
        SetLastError(0);
    }
    if unsafe { CertGetEnhancedKeyUsage(cert, 0, usage, &raw mut length) } == 0 {
        return Err(error("usage properties"));
    }
    let empty_code = unsafe { GetLastError() };
    let usage = unsafe { &*usage };
    if usage.cUsageIdentifier == 0 {
        return Ok(empty_code == NOT_FOUND);
    }
    let count = usage.cUsageIdentifier as usize;
    let start = buffer.as_ptr() as usize;
    let end = start + buffer.len() * size_of::<usize>();
    let array = usage.rgpszUsageIdentifier as usize;
    if array < start
        || !array.is_multiple_of(size_of::<usize>())
        || count > bytes / size_of::<usize>()
        || array
            .checked_add(count * size_of::<usize>())
            .is_none_or(|limit| limit > end)
    {
        return Err(LocalTrustError::Limit);
    }
    // SAFETY: pointer array lies in the aligned, initialized returned buffer.
    for oid in unsafe { std::slice::from_raw_parts(usage.rgpszUsageIdentifier, count) } {
        let pointer = *oid as usize;
        if pointer < start || pointer >= end {
            return Err(LocalTrustError::Limit);
        }
        // SAFETY: bounded OID bytes inside the native output allocation.
        let text = unsafe { std::slice::from_raw_parts(*oid, end - pointer) };
        let terminator = text
            .iter()
            .position(|b| *b == 0)
            .ok_or(LocalTrustError::Limit)?;
        if matches!(&text[..terminator], b"1.3.6.1.5.5.7.3.1" | b"2.5.29.37.0") {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn start(state: Arc<State>) -> Result<Arc<Control>, LocalTrustError> {
    start_with(state, Collections::open)
}
fn start_with(
    state: Arc<State>,
    open: impl Fn() -> Result<Collections, LocalTrustError> + Send + 'static,
) -> Result<Arc<Control>, LocalTrustError> {
    let changed = Arc::new(Event::new()?);
    let stop = Arc::new(Event::new()?);
    let worker_changed = changed.clone();
    let worker_stop = stop.clone();
    std::thread::Builder::new()
        .name("windows-local-trust".into())
        .spawn(move || {
            let result = worker(&state, &worker_changed, &worker_stop, &open);
            state.publish(Err(result.err().unwrap_or(LocalTrustError::Closed)));
        })
        .map_err(|_| LocalTrustError::Worker)?;
    Ok(Arc::new(Control { changed, stop }))
}
fn worker(
    state: &State,
    changed: &Event,
    stop: &Event,
    open: &impl Fn() -> Result<Collections, LocalTrustError>,
) -> Result<(), LocalTrustError> {
    let mut collections = None;
    loop {
        if stop.is_signaled() {
            return Ok(());
        }
        let refresh = (|| {
            if collections.is_none() {
                let stores = open()?;
                stores.control(CERT_STORE_CTRL_NOTIFY_CHANGE, changed)?;
                collections = Some(stores);
            }
            state.changed();
            changed.reset()?;
            let stores = collections.as_ref().ok_or(LocalTrustError::Worker)?;
            stores.control(CERT_STORE_CTRL_RESYNC, changed)?;
            stores.read(state.epoch.load(Ordering::Acquire))
        })();
        let failed = refresh.is_err();
        state.publish(refresh);
        if failed {
            collections = None;
        }
        let handles = [stop.0, changed.0];
        // SAFETY: worker-owned references keep both event handles alive. Each wait
        // handles one change; resync/rearm occurs before waiting for the next one.
        let result = unsafe {
            WaitForMultipleObjects(2, handles.as_ptr(), 0, if failed { 1000 } else { INFINITE })
        };
        if result == WAIT_OBJECT_0 {
            return Ok(());
        }
        if result != WAIT_OBJECT_0 + 1 && result != WAIT_TIMEOUT {
            return Err(error("event wait"));
        }
    }
}

#[cfg(test)]
mod tests;
