use super::*;
use crate::LocalTrustMonitor;
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::PKey,
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, KeyUsage},
    },
};
use std::{
    sync::atomic::AtomicUsize,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Security::Cryptography::{
    CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_CTRL_COMMIT, CERT_STORE_PROV_REG,
    CertAddEncodedCertificateToStore, CertDeleteCertificateFromStore, CertSetEnhancedKeyUsage,
    X509_ASN_ENCODING,
};
use windows_sys::Win32::System::Registry::*;
#[path = "../../tests/support/ctl.rs"]
mod ctl_fixture;

struct TemporaryRegistry {
    path: String,
    key: HKEY,
}
impl TemporaryRegistry {
    fn new() -> Self {
        static SERIAL: AtomicUsize = AtomicUsize::new(1);
        let path = format!(
            "Software\\TransmogLocalTrustTests\\{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut key = ptr::null_mut();
        let mut disposition = 0;
        // SAFETY: unique, explicitly owned HKCU test namespace, never a trust store.
        assert_eq!(
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    name.as_ptr(),
                    0,
                    ptr::null(),
                    0,
                    KEY_ALL_ACCESS,
                    ptr::null(),
                    &raw mut key,
                    &raw mut disposition,
                )
            },
            0
        );
        assert_eq!(
            disposition, REG_CREATED_NEW_KEY,
            "Never reuse an unrelated registry tree"
        );
        Self { path, key }
    }
    fn writer(&self) -> Store {
        // SAFETY: test owns the registry key; provider duplicates this handle.
        let handle = unsafe { CertOpenStore(CERT_STORE_PROV_REG, 0, 0, 0, self.key.cast()) };
        assert!(!handle.is_null());
        Store(handle)
    }
}
impl Drop for TemporaryRegistry {
    fn drop(&mut self) {
        let name: Vec<u16> = self.path.encode_utf16().chain(Some(0)).collect();
        // SAFETY: delete only the unique tree whose creation we verified, after stores close.
        unsafe {
            RegCloseKey(self.key);
            RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr());
        }
    }
}
fn reader(path: &str) -> Result<Store, LocalTrustError> {
    let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut key = ptr::null_mut();
    // SAFETY: read-only handle to the caller-owned temporary namespace.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_READ, &raw mut key) } != 0 {
        return Err(error("test registry open"));
    }
    let handle = unsafe {
        CertOpenStore(
            CERT_STORE_PROV_REG,
            0,
            0,
            CERT_STORE_READONLY_FLAG,
            key.cast(),
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if handle.is_null() {
        return Err(error("test store open"));
    }
    let child = Store(handle);
    let collection = Store::collection()?;
    if unsafe { CertAddStoreToCollection(collection.0, child.0, 0, 0) } == 0 {
        return Err(error("test collection add"));
    }
    Ok(collection)
}
fn certificate() -> Vec<u8> {
    certificate_with_digest(MessageDigest::sha256())
}
fn certificate_with_digest(digest: MessageDigest) -> Vec<u8> {
    let key = PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "Isolated collection notification fixture")
        .unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_serial_number(&BigNum::from_u32(77).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(&name).unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    cert.append_extension(&BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    cert.append_extension(&KeyUsage::new().key_cert_sign().build().unwrap())
        .unwrap();
    cert.sign(&key, digest).unwrap();
    cert.build().to_der().unwrap()
}
fn add(store: &Store, der: &[u8]) -> Certificate {
    let mut context = ptr::null_mut();
    // SAFETY: valid owned DER, test store, native setter copies input and returns an owned context.
    assert_ne!(
        unsafe {
            CertAddEncodedCertificateToStore(
                store.0,
                X509_ASN_ENCODING,
                der.as_ptr(),
                u32::try_from(der.len()).unwrap(),
                CERT_STORE_ADD_REPLACE_EXISTING,
                &raw mut context,
            )
        },
        0
    );
    Certificate(context)
}
fn commit(store: &Store) {
    assert_ne!(
        unsafe { CertControlStore(store.0, 0, CERT_STORE_CTRL_COMMIT, ptr::null()) },
        0
    );
}

#[test]
fn untrusted_ctls_match_signature_identifiers_and_certificate_hashes_without_der_entries() {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_SIGNATURE_HASH_PROP_ID, CertCreateCertificateContext,
        CertGetCertificateContextProperty,
    };
    let der = certificate();
    let other = certificate();
    let context = Certificate(unsafe {
        CertCreateCertificateContext(
            X509_ASN_ENCODING,
            der.as_ptr(),
            u32::try_from(der.len()).unwrap(),
        )
    });
    assert!(!context.0.is_null());
    let mut signature = vec![0; 128];
    let mut length = u32::try_from(signature.len()).unwrap();
    assert_ne!(
        unsafe {
            CertGetCertificateContextProperty(
                context.0,
                CERT_SIGNATURE_HASH_PROP_ID,
                signature.as_mut_ptr().cast(),
                &raw mut length,
            )
        },
        0
    );
    signature.truncate(length as usize);
    for (is_signature, hash) in [
        (true, signature),
        (
            false,
            boring::hash::hash(MessageDigest::sha1(), &der)
                .unwrap()
                .to_vec(),
        ),
    ] {
        let bytes = ctl_fixture::ctl(is_signature, &[hash]);
        let snapshot = crate::CtlDistrust::new(&[bytes]).unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot.contains(&der).unwrap());
        assert!(!snapshot.contains(&other).unwrap());
    }
    assert!(crate::CtlDistrust::new(&[vec![1, 2, 3]]).is_err());
}
fn await_inventory(
    monitor: &LocalTrustMonitor,
    matches: impl Fn(&LocalCertificates) -> bool,
) -> Arc<LocalCertificates> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(value) = monitor.current()
            && matches(&value)
        {
            return value;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Notification/resync did not arrive: {:?}",
            monitor.current()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn registry_collections_notify_add_remove_property_changes_and_stop_cleanly() {
    let root = TemporaryRegistry::new();
    let intermediary = TemporaryRegistry::new();
    let people = TemporaryRegistry::new();
    let disallowed = TemporaryRegistry::new();
    let ctls = TemporaryRegistry::new();
    let paths = [
        root.path.clone(),
        intermediary.path.clone(),
        people.path.clone(),
        disallowed.path.clone(),
        ctls.path.clone(),
    ];
    let state = Arc::new(State::new());
    let control = start_with(state.clone(), move || {
        Ok(Collections {
            roots: reader(&paths[0])?,
            intermediates: reader(&paths[1])?,
            leaves: reader(&paths[2])?,
            blocked: reader(&paths[3])?,
            ctl_registry: Some({
                let mut key = ptr::null_mut();
                let name: Vec<u16> = paths[4].encode_utf16().chain(Some(0)).collect();
                assert_eq!(
                    unsafe {
                        RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_READ, &raw mut key)
                    },
                    0
                );
                CtlRegistry(key, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET)
            }),
            watches: Vec::new(),
        })
    })
    .unwrap();
    let monitor = LocalTrustMonitor {
        state: state.clone(),
        control: Some(control),
    };
    let initial = monitor.wait_ready(Duration::from_secs(5)).unwrap();
    assert_eq!(initial.anchors, [] as [std::vec::Vec<u8>; 0]);
    let writer = root.writer();
    let der = certificate();
    let cert = add(&writer, &der);
    commit(&writer);
    let added = await_inventory(&monitor, |s| s.anchors == [der.clone()]);
    assert!(added.generation > initial.generation);
    assert_ne!(added.sha256, initial.sha256);
    let mut oid = b"1.3.6.1.5.5.7.3.2\0".to_vec(); // clientAuth, not serverAuth
    let mut oids = [oid.as_mut_ptr()];
    let usage = CTL_USAGE {
        cUsageIdentifier: 1,
        rgpszUsageIdentifier: oids.as_mut_ptr(),
    };
    assert_ne!(
        unsafe { CertSetEnhancedKeyUsage(cert.0, &raw const usage) },
        0
    );
    commit(&writer);
    let restricted = await_inventory(&monitor, |s| {
        s.generation > added.generation && s.anchors.is_empty()
    });
    assert_eq!(restricted.issuer_roots, std::slice::from_ref(&der));
    let blocked_writer = disallowed.writer();
    let blocked = add(&blocked_writer, &der);
    commit(&blocked_writer);
    let denied = await_inventory(&monitor, |s| s.disallowed == [der.clone()]);
    assert!(denied.generation > restricted.generation);
    exercise_ctl_registry(&ctls, &monitor, &der);
    let owned = cert.0;
    std::mem::forget(cert);
    assert_ne!(unsafe { CertDeleteCertificateFromStore(owned) }, 0);
    commit(&writer);
    let owned = blocked.0;
    std::mem::forget(blocked);
    assert_ne!(unsafe { CertDeleteCertificateFromStore(owned) }, 0);
    commit(&blocked_writer);
    await_inventory(&monitor, |s| {
        s.generation > denied.generation && s.disallowed.is_empty()
    });
    drop(monitor);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !matches!(state.current(), Err(LocalTrustError::Closed)) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn production_registry_selection_is_read_only_and_monitored() {
    let monitor = LocalTrustMonitor::start().unwrap();
    let snapshot = monitor.wait_ready(Duration::from_secs(5)).unwrap();
    assert_eq!(snapshot.sha256.len(), 64);
    assert_eq!(snapshot.generation, monitor.generation());
    assert!(
        !crate::CtlDistrust::new(&snapshot.disallowed_ctls)
            .unwrap()
            .contains(&certificate())
            .unwrap()
    );
    let chromium = LocalTrustMonitor::start_chromium().unwrap();
    assert_eq!(
        chromium
            .wait_ready(Duration::from_secs(5))
            .unwrap()
            .disallowed_ctls,
        [] as [std::vec::Vec<u8>; 0]
    );
}

#[test]
fn registry_watch_detects_creation_of_a_previously_absent_distrust_store() {
    let registry = TemporaryRegistry::new();
    let path = format!("{}\\missing\\Disallowed", registry.path);
    let event = Event::new().unwrap();
    let watch = CtlRegistry::watch(HKEY_CURRENT_USER, &path).unwrap();
    watch.notify(&event).unwrap();
    let mut key = ptr::null_mut();
    assert_eq!(
        unsafe {
            RegCreateKeyExW(
                registry.key,
                windows_sys::core::w!("missing\\Disallowed"),
                0,
                ptr::null(),
                0,
                KEY_ALL_ACCESS,
                ptr::null(),
                &raw mut key,
                ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(unsafe { WaitForSingleObject(event.0, 2000) }, WAIT_OBJECT_0);
    drop(watch);
    event.reset().unwrap();
    let watch = CtlRegistry::watch(HKEY_CURRENT_USER, &path).unwrap();
    watch.notify(&event).unwrap();
    assert_eq!(
        unsafe {
            RegSetValueExW(
                key,
                windows_sys::core::w!("fixture"),
                0,
                REG_BINARY,
                [1_u8].as_ptr(),
                1,
            )
        },
        0
    );
    assert_eq!(unsafe { WaitForSingleObject(event.0, 2000) }, WAIT_OBJECT_0);
    unsafe { RegCloseKey(key) };
}

#[test]
fn signature_ctls_support_sha384_identifiers_without_certificate_thumbprints() {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_SIGNATURE_HASH_PROP_ID, CertCreateCertificateContext,
        CertGetCertificateContextProperty,
    };
    let der = certificate_with_digest(MessageDigest::sha384());
    // CERT_SIGNATURE_HASH_PROP_ID is the digest of the certificate's signed TBS,
    // not a thumbprint of its complete DER or a hash of its signature bits.
    let context = Certificate(unsafe {
        CertCreateCertificateContext(
            X509_ASN_ENCODING,
            der.as_ptr(),
            u32::try_from(der.len()).unwrap(),
        )
    });
    let mut hash = vec![0; 48];
    let mut length = 48;
    assert_ne!(
        unsafe {
            CertGetCertificateContextProperty(
                context.0,
                CERT_SIGNATURE_HASH_PROP_ID,
                hash.as_mut_ptr().cast(),
                &raw mut length,
            )
        },
        0
    );
    assert_eq!(length, 48);
    let ctl = crate::CtlDistrust::new(&[ctl_fixture::ctl(true, &[hash])]).unwrap();
    assert!(ctl.contains(&der).unwrap());
    assert!(!ctl.contains(&certificate()).unwrap());
}

fn exercise_ctl_registry(ctls: &TemporaryRegistry, monitor: &LocalTrustMonitor, der: &[u8]) {
    let mut key = ptr::null_mut();
    assert_eq!(
        unsafe {
            RegCreateKeyExW(
                ctls.key,
                windows_sys::core::w!("AuthRoot\\AutoUpdate"),
                0,
                ptr::null(),
                0,
                KEY_ALL_ACCESS,
                ptr::null(),
                &raw mut key,
                ptr::null_mut(),
            )
        },
        0
    );
    let bytes = ctl_fixture::ctl(
        false,
        &[boring::hash::hash(MessageDigest::sha1(), der)
            .unwrap()
            .to_vec()],
    );
    let write = |bytes: &[u8]| {
        assert_eq!(
            unsafe {
                RegSetValueExW(
                    key,
                    windows_sys::core::w!("DisallowedCertEncodedCtl"),
                    0,
                    REG_BINARY,
                    bytes.as_ptr(),
                    u32::try_from(bytes.len()).unwrap(),
                )
            },
            0
        );
    };
    write(&bytes);
    let ctl_snapshot = await_inventory(monitor, |s| s.disallowed_ctls == [bytes.clone()]);
    assert!(
        crate::CtlDistrust::new(&ctl_snapshot.disallowed_ctls)
            .unwrap()
            .contains(der)
            .unwrap()
    );
    write(&[1, 2, 3]);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !matches!(
        monitor.current(),
        Err(LocalTrustError::Windows {
            operation: "untrusted CTL decoding",
            ..
        })
    ) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Restore valid data before removal, exercising recovery from a failed read.
    write(&bytes);
    await_inventory(monitor, |s| {
        s.generation > ctl_snapshot.generation && s.disallowed_ctls == [bytes.clone()]
    });
    assert_eq!(
        unsafe { RegDeleteValueW(key, windows_sys::core::w!("DisallowedCertEncodedCtl")) },
        0
    );
    unsafe { RegCloseKey(key) };
    await_inventory(monitor, |s| s.disallowed_ctls.is_empty());
    assert_eq!(ctl_snapshot.disallowed_ctls, [bytes]);
}
