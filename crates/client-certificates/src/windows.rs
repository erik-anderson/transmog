use crate::{
    CertificateError, DigestAlgorithm, KeyAlgorithm, SignaturePadding, SigningIdentity,
    UserCertificate,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::{CRYPT_E_NOT_FOUND, GetLastError},
    Security::Cryptography::{
        BCRYPT_PKCS1_PADDING_INFO, BCRYPT_PSS_PADDING_INFO, BCRYPT_SHA1_ALGORITHM,
        BCRYPT_SHA256_ALGORITHM, BCRYPT_SHA384_ALGORITHM, BCRYPT_SHA512_ALGORITHM,
        CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL, CERT_CHAIN_CONTEXT, CERT_CHAIN_DISABLE_AIA,
        CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE, CERT_CHAIN_PARA, CERT_CONTEXT,
        CERT_FIND_SHA1_HASH, CERT_KEY_PROV_INFO_PROP_ID, CERT_NCRYPT_KEY_SPEC,
        CERT_PUBLIC_KEY_INFO, CERT_SHA1_HASH_PROP_ID, CERT_STORE_OPEN_EXISTING_FLAG,
        CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_CURRENT_USER,
        CRYPT_INTEGER_BLOB, CRYPT_KEY_PROV_INFO, CRYPT_MACHINE_KEYSET, CertCloseStore,
        CertComparePublicKeyInfo, CertDuplicateCertificateContext, CertEnumCertificatesInStore,
        CertFindCertificateInStore, CertFreeCertificateChain, CertFreeCertificateContext,
        CertGetCertificateChain, CertGetCertificateContextProperty, CertOpenStore,
        CryptExportPublicKeyInfo, HCERTSTORE, MS_KEY_STORAGE_PROVIDER, NCRYPT_ALGORITHM_PROPERTY,
        NCRYPT_ALLOW_SIGNING_FLAG, NCRYPT_ECC_CURVE_NAME_PROPERTY, NCRYPT_HANDLE,
        NCRYPT_KEY_HANDLE, NCRYPT_KEY_USAGE_PROPERTY, NCRYPT_PAD_PKCS1_FLAG, NCRYPT_PAD_PSS_FLAG,
        NCRYPT_PROV_HANDLE, NCRYPT_SILENT_FLAG, NCryptFreeObject, NCryptGetProperty, NCryptOpenKey,
        NCryptOpenStorageProvider, NCryptSignHash, X509_ASN_ENCODING,
    },
};

const MAX_CERT_BYTES: usize = 1024 * 1024;
const MAX_PROPERTY_BYTES: usize = 64 * 1024;
const MAX_INVENTORY_BYTES: usize = 32 * 1024 * 1024;
const SOFTWARE_PROVIDER: &str = "Microsoft Software Key Storage Provider";

struct Store(HCERTSTORE);
impl Store {
    fn open() -> Result<Self, CertificateError> {
        // SAFETY: the provider constant denotes SYSTEM_W; the terminated name
        // lives throughout the synchronous read-only CURRENT_USER open call.
        let handle = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                0,
                0,
                CERT_SYSTEM_STORE_CURRENT_USER
                    | CERT_STORE_OPEN_EXISTING_FLAG
                    | CERT_STORE_READONLY_FLAG,
                windows_sys::core::w!("MY").cast(),
            )
        };
        if handle.is_null() {
            Err(CertificateError)
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: this is our owned store; all contexts are freed before it closes.
        unsafe {
            CertCloseStore(self.0, 0);
        }
    }
}
struct Certificate(*mut CERT_CONTEXT);
impl Drop for Certificate {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this context reference is owned and freed exactly once.
            unsafe {
                CertFreeCertificateContext(self.0);
            }
        }
    }
}
struct NativeKey {
    key: NCRYPT_KEY_HANDLE,
    provider: NCRYPT_PROV_HANDLE,
}
impl Drop for NativeKey {
    fn drop(&mut self) {
        // SAFETY: both handles are ours, never cached in a certificate; release
        // the key before its provider, including on partial acquisition failure.
        unsafe {
            if self.key != 0 {
                NCryptFreeObject(self.key);
            }
            if self.provider != 0 {
                NCryptFreeObject(self.provider);
            }
        }
    }
}
struct Chain(*mut CERT_CHAIN_CONTEXT);
impl Drop for Chain {
    fn drop(&mut self) {
        // SAFETY: CertGetCertificateChain returned this owned context.
        unsafe {
            CertFreeCertificateChain(self.0);
        }
    }
}

fn der(certificate: &Certificate) -> Result<Vec<u8>, CertificateError> {
    // SAFETY: the context is owned; its encoded public bytes live with it.
    let context = unsafe { &*certificate.0 };
    let length = context.cbCertEncoded as usize;
    if context.pbCertEncoded.is_null() || length == 0 || length > MAX_CERT_BYTES {
        return Err(CertificateError);
    }
    // SAFETY: Windows promises cbCertEncoded initialized bytes in this context.
    Ok(unsafe { std::slice::from_raw_parts(context.pbCertEncoded, length) }.to_vec())
}

// Aligned storage for native structures followed by their pointed-to data.
fn storage(bytes: usize) -> Result<Vec<usize>, CertificateError> {
    if bytes == 0 || bytes > MAX_PROPERTY_BYTES {
        return Err(CertificateError);
    }
    Ok(vec![0; bytes.div_ceil(size_of::<usize>())])
}
fn wide_in_buffer(
    buffer: &[usize],
    bytes: usize,
    pointer: *const u16,
) -> Result<Vec<u16>, CertificateError> {
    if bytes > std::mem::size_of_val(buffer) {
        return Err(CertificateError);
    }
    let start = buffer.as_ptr() as usize;
    let end = start + bytes;
    let address = pointer as usize;
    if address < start || address >= end || !address.is_multiple_of(align_of::<u16>()) {
        return Err(CertificateError);
    }
    // SAFETY: validated aligned pointer is confined to our initialized allocation.
    let units = unsafe { std::slice::from_raw_parts(pointer, (end - address) / 2) };
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .ok_or(CertificateError)?;
    if length == 0 {
        return Err(CertificateError);
    }
    Ok(units[..=length].to_vec())
}
fn container(certificate: &Certificate) -> Result<(Vec<u16>, u32), CertificateError> {
    let mut bytes = 0;
    // SAFETY: null output is the documented size query on a live certificate.
    if unsafe {
        CertGetCertificateContextProperty(
            certificate.0,
            CERT_KEY_PROV_INFO_PROP_ID,
            ptr::null_mut(),
            &raw mut bytes,
        )
    } == 0
    {
        return Err(CertificateError);
    }
    let mut data = storage(bytes as usize)?;
    // SAFETY: data is aligned, initialized, and at least bytes writable bytes.
    if unsafe {
        CertGetCertificateContextProperty(
            certificate.0,
            CERT_KEY_PROV_INFO_PROP_ID,
            data.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    } == 0
        || (bytes as usize) < size_of::<CRYPT_KEY_PROV_INFO>()
        || bytes as usize > std::mem::size_of_val(data.as_slice())
    {
        return Err(CertificateError);
    }
    // SAFETY: successful property lookup initialized the aligned structure.
    let info = unsafe { &*data.as_ptr().cast::<CRYPT_KEY_PROV_INFO>() };
    let provider = wide_in_buffer(&data, bytes as usize, info.pwszProvName)?;
    if info.dwProvType != 0
        || !matches!(info.dwKeySpec, 0 | 1 | 2 | CERT_NCRYPT_KEY_SPEC)
        || info.dwFlags & CRYPT_MACHINE_KEYSET != 0
        || info.cProvParam != 0
        || !info.rgProvParam.is_null()
        || String::from_utf16(&provider[..provider.len() - 1])
            .ok()
            .as_deref()
            != Some(SOFTWARE_PROVIDER)
    {
        return Err(CertificateError);
    }
    // This property's dwKeySpec is NCryptOpenKey's legacy key spec, distinct
    // from CryptAcquireCertificatePrivateKey's CERT_NCRYPT_KEY_SPEC output tag.
    let spec = if info.dwKeySpec == CERT_NCRYPT_KEY_SPEC {
        0
    } else {
        info.dwKeySpec
    };
    Ok((
        wide_in_buffer(&data, bytes as usize, info.pwszContainerName)?,
        spec,
    ))
}
fn property(key: NCRYPT_HANDLE, name: *const u16) -> Result<Vec<u8>, CertificateError> {
    let mut length = 0;
    // SAFETY: owned handle and static terminated property; null output queries size.
    if unsafe {
        NCryptGetProperty(
            key,
            name,
            ptr::null_mut(),
            0,
            &raw mut length,
            NCRYPT_SILENT_FLAG,
        )
    } != 0
        || length == 0
        || length > 4096
    {
        return Err(CertificateError);
    }
    let mut data = vec![0; length as usize];
    let capacity = length;
    // SAFETY: data has exactly capacity initialized writable bytes.
    if unsafe {
        NCryptGetProperty(
            key,
            name,
            data.as_mut_ptr(),
            capacity,
            &raw mut length,
            NCRYPT_SILENT_FLAG,
        )
    } != 0
        || length > capacity
    {
        return Err(CertificateError);
    }
    data.truncate(length as usize);
    Ok(data)
}
fn algorithm(key: NCRYPT_KEY_HANDLE) -> Result<KeyAlgorithm, CertificateError> {
    let usage = property(key, NCRYPT_KEY_USAGE_PROPERTY)?;
    if usage.len() != 4
        || u32::from_le_bytes(usage.try_into().map_err(|_| CertificateError)?)
            & NCRYPT_ALLOW_SIGNING_FLAG
            == 0
    {
        return Err(CertificateError);
    }
    match property_text(key, NCRYPT_ALGORITHM_PROPERTY)?.as_str() {
        "RSA" => Ok(KeyAlgorithm::Rsa),
        "ECDSA_P256" => Ok(KeyAlgorithm::EcdsaP256),
        "ECDSA_P384" => Ok(KeyAlgorithm::EcdsaP384),
        "ECDSA_P521" => Ok(KeyAlgorithm::EcdsaP521),
        "ECDSA" => named_curve(&property_text(key, NCRYPT_ECC_CURVE_NAME_PROPERTY)?),
        _ => Err(CertificateError),
    }
}
fn property_text(key: NCRYPT_HANDLE, name: *const u16) -> Result<String, CertificateError> {
    let bytes = property(key, name)?;
    if bytes.len() % 2 != 0 {
        return Err(CertificateError);
    }
    let units = bytes
        .chunks_exact(2)
        .map(|x| u16::from_le_bytes([x[0], x[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(units.strip_suffix(&[0]).ok_or(CertificateError)?)
        .map_err(|_| CertificateError)
}
fn named_curve(name: &str) -> Result<KeyAlgorithm, CertificateError> {
    // Only these Windows curve names identify the supported NIST curves. Bit
    // length alone would also admit unrelated curves, such as brainpool P-256.
    match name {
        "nistP256" | "secP256r1" | "x962P256v1" => Ok(KeyAlgorithm::EcdsaP256),
        "nistP384" | "secP384r1" => Ok(KeyAlgorithm::EcdsaP384),
        "nistP521" | "secP521r1" => Ok(KeyAlgorithm::EcdsaP521),
        _ => Err(CertificateError),
    }
}
fn acquire(
    certificate: &Certificate,
    expected: Option<&SigningIdentity>,
) -> Result<(NativeKey, Vec<u16>, u32, KeyAlgorithm), CertificateError> {
    let (container, spec) = container(certificate)?;
    if expected
        .is_some_and(|identity| identity.container != container || identity.legacy_spec != spec)
    {
        return Err(CertificateError);
    }
    let mut native = NativeKey {
        key: 0,
        provider: 0,
    };
    // SAFETY: returned handles are owned by native; the static provider name is
    // the supported software provider, vetted before touching any private key.
    if unsafe { NCryptOpenStorageProvider(&raw mut native.provider, MS_KEY_STORAGE_PROVIDER, 0) }
        != 0
    {
        return Err(CertificateError);
    }
    // SAFETY: provider is live, container is terminated and owned for the call.
    // No machine-key flag, cache, healing, fallback provider or UI is permitted.
    if unsafe {
        NCryptOpenKey(
            native.provider,
            &raw mut native.key,
            container.as_ptr(),
            spec,
            NCRYPT_SILENT_FLAG,
        )
    } != 0
    {
        return Err(CertificateError);
    }
    let algorithm = algorithm(native.key)?;
    if expected.is_some_and(|identity| identity.algorithm != algorithm) {
        return Err(CertificateError);
    }
    let mut bytes = 0;
    // SAFETY: only PUBLIC information is requested from the owned CNG key. Null
    // output queries allocation size; no private-key export API is used.
    if unsafe {
        CryptExportPublicKeyInfo(
            native.key,
            CERT_NCRYPT_KEY_SPEC,
            X509_ASN_ENCODING,
            ptr::null_mut(),
            &raw mut bytes,
        )
    } == 0
    {
        return Err(CertificateError);
    }
    let mut public = storage(bytes as usize)?;
    // SAFETY: initialized aligned allocation covers bytes; exported pointer fields
    // stay inside it and are consumed only during this synchronous comparison.
    if unsafe {
        CryptExportPublicKeyInfo(
            native.key,
            CERT_NCRYPT_KEY_SPEC,
            X509_ASN_ENCODING,
            public.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    } == 0
        || (bytes as usize) < size_of::<CERT_PUBLIC_KEY_INFO>()
        || bytes as usize > std::mem::size_of_val(public.as_slice())
    {
        return Err(CertificateError);
    }
    // SAFETY: certificate owns pCertInfo; export initialized CERT_PUBLIC_KEY_INFO.
    if unsafe {
        CertComparePublicKeyInfo(
            X509_ASN_ENCODING,
            &raw const (*(*certificate.0).pCertInfo).SubjectPublicKeyInfo,
            public.as_ptr().cast(),
        )
    } == 0
    {
        return Err(CertificateError);
    }
    Ok((native, container, spec, algorithm))
}

fn public_chain(
    store: &Store,
    certificate: &Certificate,
) -> Result<Vec<Vec<u8>>, CertificateError> {
    let parameters = CERT_CHAIN_PARA {
        cbSize: u32::try_from(size_of::<CERT_CHAIN_PARA>()).map_err(|_| CertificateError)?,
        ..Default::default()
    };
    let mut pointer = ptr::null_mut();
    // SAFETY: all inputs are live, output is an owned chain. Use only local
    // issuers; do not download AIA, revocation URLs, or automatic trusted roots.
    if unsafe {
        CertGetCertificateChain(
            ptr::null_mut(),
            certificate.0,
            ptr::null(),
            store.0,
            &raw const parameters,
            CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL
                | CERT_CHAIN_DISABLE_AIA
                | CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE,
            ptr::null(),
            &raw mut pointer,
        )
    } == 0
        || pointer.is_null()
    {
        return Err(CertificateError);
    }
    let chain = Chain(pointer);
    // SAFETY: chain-owned structures/arrays remain valid until chain is dropped.
    let context = unsafe { &*chain.0 };
    if context.cChain == 0 || context.rgpChain.is_null() {
        return Err(CertificateError);
    }
    // SAFETY: a successful context contains cChain pointers, at least one here.
    let simple = unsafe { *context.rgpChain };
    if simple.is_null() {
        return Err(CertificateError);
    }
    // SAFETY: first chain is borrowed from our owned context.
    let simple = unsafe { &*simple };
    if simple.cElement == 0 || simple.cElement > 32 || simple.rgpElement.is_null() {
        return Err(CertificateError);
    }
    // SAFETY: array contains cElement initialized pointers owned by the chain.
    let elements =
        unsafe { std::slice::from_raw_parts(simple.rgpElement, simple.cElement as usize) };
    let mut output = Vec::new();
    for &element in elements {
        if element.is_null() {
            return Err(CertificateError);
        }
        // SAFETY: element owns a borrowed certificate; duplicate it to use the
        // same RAII byte-copy helper without releasing the chain's reference.
        let duplicate = unsafe { CertDuplicateCertificateContext((*element).pCertContext) };
        if duplicate.is_null() {
            return Err(CertificateError);
        }
        output.push(der(&Certificate(duplicate))?);
    }
    if output.first() != Some(&der(certificate)?) {
        return Err(CertificateError);
    }
    Ok(output)
}

pub(super) fn inventory(
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<UserCertificate>, CertificateError> {
    if cancelled() {
        return Err(CertificateError);
    }
    let store = Store::open()?;
    let mut certificate = Certificate(ptr::null_mut());
    let mut identities = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total_bytes = 0;
    for _ in 0..=4096 {
        if cancelled() {
            return Err(CertificateError);
        }
        let previous = std::mem::replace(&mut certificate.0, ptr::null_mut());
        // SAFETY: enumeration consumes previous even on failure; transfer that
        // one owned reference to Windows, then own the next returned reference.
        certificate.0 = unsafe { CertEnumCertificatesInStore(store.0, previous) };
        if certificate.0.is_null() {
            // SAFETY: read error immediately after the failed enumeration call.
            if unsafe { GetLastError() } == CRYPT_E_NOT_FOUND.cast_unsigned() {
                return Ok(identities);
            }
            return Err(CertificateError);
        }
        if seen.len() >= 4096 {
            return Err(CertificateError);
        }
        let fingerprint: [u8; 32] = Sha256::digest(der(&certificate)?).into();
        if !seen.insert(fingerprint) {
            continue;
        }
        if cancelled() {
            return Err(CertificateError);
        }
        let Ok((_key, container, legacy_spec, algorithm)) = acquire(&certificate, None) else {
            continue;
        };
        if identities.len() >= 1024 {
            return Err(CertificateError);
        }
        if cancelled() {
            return Err(CertificateError);
        }
        let chain = public_chain(&store, &certificate)?;
        total_bytes += chain.iter().map(Vec::len).sum::<usize>();
        if total_bytes > MAX_INVENTORY_BYTES {
            return Err(CertificateError);
        }
        let mut lookup_hash = [0; 20];
        let mut bytes = 20;
        // SAFETY: exact-sized initialized hash output is live for this call.
        // SHA-1 is only a native store index; SHA-256 is rechecked before use.
        if unsafe {
            CertGetCertificateContextProperty(
                certificate.0,
                CERT_SHA1_HASH_PROP_ID,
                lookup_hash.as_mut_ptr().cast(),
                &raw mut bytes,
            )
        } == 0
            || bytes != 20
        {
            return Err(CertificateError);
        }
        identities.push(UserCertificate {
            chain,
            key: SigningIdentity {
                fingerprint,
                algorithm,
                container,
                legacy_spec,
                lookup_hash,
            },
        });
    }
    Err(CertificateError)
}
fn find(identity: &SigningIdentity, store: &Store) -> Result<Certificate, CertificateError> {
    let mut certificate = Certificate(ptr::null_mut());
    let hash = CRYPT_INTEGER_BLOB {
        cbData: 20,
        pbData: identity.lookup_hash.as_ptr().cast_mut(),
    };
    for _ in 0..4096 {
        let previous = std::mem::replace(&mut certificate.0, ptr::null_mut());
        // SAFETY: find consumes previous and returns an owned next context. The
        // fixed hash blob and its read-only bytes stay live for the call. This
        // is an index lookup only; acceptance still requires exact SHA-256 DER.
        certificate.0 = unsafe {
            CertFindCertificateInStore(
                store.0,
                X509_ASN_ENCODING,
                0,
                CERT_FIND_SHA1_HASH,
                ptr::from_ref(&hash).cast(),
                previous,
            )
        };
        if certificate.0.is_null() {
            return Err(CertificateError);
        }
        let hash: [u8; 32] = Sha256::digest(der(&certificate)?).into();
        if hash == identity.fingerprint {
            return Ok(certificate);
        }
    }
    Err(CertificateError)
}
pub(super) fn available(identity: &SigningIdentity) -> bool {
    Store::open()
        .and_then(|store| find(identity, &store).and_then(|cert| acquire(&cert, Some(identity))))
        .is_ok()
}
pub(super) fn sign(
    identity: &SigningIdentity,
    digest: DigestAlgorithm,
    padding: SignaturePadding,
    input: &[u8],
) -> Result<Vec<u8>, CertificateError> {
    let store = Store::open()?;
    let certificate = find(identity, &store)?;
    let (key, _, _, _) = acquire(&certificate, Some(identity))?;
    let name = match digest {
        DigestAlgorithm::Sha1 => BCRYPT_SHA1_ALGORITHM,
        DigestAlgorithm::Sha256 => BCRYPT_SHA256_ALGORITHM,
        DigestAlgorithm::Sha384 => BCRYPT_SHA384_ALGORITHM,
        DigestAlgorithm::Sha512 => BCRYPT_SHA512_ALGORITHM,
    };
    let pkcs1 = BCRYPT_PKCS1_PADDING_INFO { pszAlgId: name };
    let pss = BCRYPT_PSS_PADDING_INFO {
        pszAlgId: name,
        cbSalt: u32::try_from(input.len()).map_err(|_| CertificateError)?,
    };
    let (info, flags) = match padding {
        SignaturePadding::RsaPkcs1 => (ptr::from_ref(&pkcs1).cast(), NCRYPT_PAD_PKCS1_FLAG),
        SignaturePadding::RsaPss => (ptr::from_ref(&pss).cast(), NCRYPT_PAD_PSS_FLAG),
        SignaturePadding::Ecdsa => (ptr::null(), 0),
    };
    let length = u32::try_from(input.len()).map_err(|_| CertificateError)?;
    let mut bytes = 0;
    // SAFETY: local padding/hash and fresh key live for both synchronous calls.
    // A null output queries public signature size. UI is forbidden for signing.
    if unsafe {
        NCryptSignHash(
            key.key,
            info,
            input.as_ptr(),
            length,
            ptr::null_mut(),
            0,
            &raw mut bytes,
            flags | NCRYPT_SILENT_FLAG,
        )
    } != 0
        || bytes == 0
        || bytes > 16384
    {
        return Err(CertificateError);
    }
    let mut output = vec![0; bytes as usize];
    let capacity = bytes;
    // SAFETY: output is initialized/writable for capacity; all inputs still live.
    if unsafe {
        NCryptSignHash(
            key.key,
            info,
            input.as_ptr(),
            length,
            output.as_mut_ptr(),
            capacity,
            &raw mut bytes,
            flags | NCRYPT_SILENT_FLAG,
        )
    } != 0
        || bytes == 0
        || bytes > capacity
    {
        return Err(CertificateError);
    }
    output.truncate(bytes as usize);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_curves_match_only_supported_nist_names() {
        for name in ["nistP256", "secP256r1", "x962P256v1"] {
            assert_eq!(named_curve(name), Ok(KeyAlgorithm::EcdsaP256));
        }
        for name in ["nistP384", "secP384r1"] {
            assert_eq!(named_curve(name), Ok(KeyAlgorithm::EcdsaP384));
        }
        for name in ["nistP521", "secP521r1"] {
            assert_eq!(named_curve(name), Ok(KeyAlgorithm::EcdsaP521));
        }
        for name in [
            "",
            "brainpoolP256r1",
            "secP256k1",
            "curve25519",
            "nistP256\0",
        ] {
            assert_eq!(named_curve(name), Err(CertificateError));
        }
    }

    #[test]
    fn native_strings_stay_within_returned_bytes_and_require_termination() {
        let mut data = storage(12).unwrap();
        let pointer = data.as_mut_ptr().cast::<u16>();
        for index in 0..6 {
            // SAFETY: this test's initialized aligned allocation holds at least
            // twelve writable bytes. Simulate a native nonterminated string.
            unsafe {
                pointer.add(index).write(b'a'.into());
            }
        }
        // Allocation padding is zero, but outside the native returned byte count.
        assert!(wide_in_buffer(&data, 12, pointer).is_err());
        // SAFETY: the sixth u16 is still inside our owned allocation.
        unsafe {
            pointer.add(5).write(0);
        }
        assert_eq!(
            wide_in_buffer(&data, 12, pointer).unwrap(),
            vec![97, 97, 97, 97, 97, 0]
        );
        assert!(wide_in_buffer(&data, 12, ptr::null()).is_err());
        assert!(wide_in_buffer(&data, 12, pointer.cast::<u8>().wrapping_add(1).cast()).is_err());
        assert!(wide_in_buffer(&data, 12, pointer.wrapping_add(6)).is_err());
        assert!(wide_in_buffer(&data, usize::MAX, pointer).is_err());
    }
}
