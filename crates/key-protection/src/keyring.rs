use crate::{envelope, invalid, payload, unavailable};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::fmt::Write as _;
use std::io;
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
const BACKEND: u8 = 2;
#[cfg(target_os = "linux")]
const BACKEND: u8 = 3;
#[cfg(all(test, not(any(target_os = "macos", target_os = "linux"))))]
const BACKEND: u8 = 3;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SERVICE: &str = "Transmog interception CA wrapping key";
const ID_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn entry(id: &[u8]) -> io::Result<::keyring::Entry> {
    let mut name = String::with_capacity(ID_BYTES * 2);
    for byte in id {
        write!(name, "{byte:02x}").expect("String write succeeds");
    }
    ::keyring::Entry::new(SERVICE, &name).map_err(|_| unavailable())
}

trait WrappingStore {
    fn set(&self, id: &[u8], key: &[u8]) -> io::Result<()>;
    fn get(&self, id: &[u8]) -> io::Result<Zeroizing<Vec<u8>>>;
    fn delete(&self, id: &[u8]) -> io::Result<()>;
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct NativeStore;
#[cfg(any(target_os = "macos", target_os = "linux"))]
impl WrappingStore for NativeStore {
    fn set(&self, id: &[u8], key: &[u8]) -> io::Result<()> {
        entry(id)?.set_secret(key).map_err(|_| unavailable())
    }
    fn get(&self, id: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        entry(id)?
            .get_secret()
            .map(Zeroizing::new)
            .map_err(|_| unavailable())
    }
    fn delete(&self, id: &[u8]) -> io::Result<()> {
        match entry(id)?.delete_credential() {
            Ok(()) | Err(::keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}
fn split(bytes: &[u8]) -> io::Result<(&[u8], &[u8], &[u8])> {
    let payload = payload(bytes, BACKEND)?;
    if payload.len() < ID_BYTES + NONCE_BYTES + 16 {
        return Err(invalid());
    }
    Ok((
        &payload[..ID_BYTES],
        &payload[ID_BYTES..ID_BYTES + NONCE_BYTES],
        &payload[ID_BYTES + NONCE_BYTES..],
    ))
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn protect(bytes: &[u8]) -> io::Result<Vec<u8>> {
    protect_with(bytes, &NativeStore)
}
fn protect_with(bytes: &[u8], store: &impl WrappingStore) -> io::Result<Vec<u8>> {
    let mut key = Zeroizing::new([0_u8; 32]);
    let mut id = [0_u8; ID_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(key.as_mut()).map_err(|_| unavailable())?;
    getrandom::fill(&mut id).map_err(|_| unavailable())?;
    getrandom::fill(&mut nonce).map_err(|_| unavailable())?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| invalid())?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: bytes,
                aad: &id,
            },
        )
        .map_err(|_| invalid())?;
    store.set(&id, key.as_ref())?;
    // Verify persistence before publishing a file dependent on this credential.
    match store.get(&id) {
        Ok(stored) if stored.as_slice() == key.as_ref() => {}
        _ => {
            let _ = store.delete(&id);
            return Err(unavailable());
        }
    }
    let mut data = id.to_vec();
    data.extend_from_slice(&nonce);
    data.extend_from_slice(&ciphertext);
    Ok(envelope(BACKEND, &data))
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn unprotect(bytes: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
    unprotect_with(bytes, &NativeStore)
}
fn unprotect_with(bytes: &[u8], store: &impl WrappingStore) -> io::Result<Zeroizing<Vec<u8>>> {
    let (id, nonce, ciphertext) = split(bytes)?;
    let key = store.get(id)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| invalid())?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: id,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| invalid())
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn forget(bytes: &[u8]) -> io::Result<()> {
    let (id, _, _) = split(bytes)?;
    NativeStore.delete(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Store(std::sync::Mutex<std::collections::HashMap<Vec<u8>, Vec<u8>>>);
    impl WrappingStore for Store {
        fn set(&self, id: &[u8], key: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().insert(id.to_vec(), key.to_vec());
            Ok(())
        }
        fn get(&self, id: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
            self.0
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(unavailable)
        }
        fn delete(&self, id: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().remove(id);
            Ok(())
        }
    }
    #[test]
    fn wrapping_key_stays_out_of_the_file_and_authenticates_all_material() {
        let store = Store::default();
        let plaintext = b"-----BEGIN PRIVATE KEY-----\nAES fixture only\n";
        let protected = protect_with(plaintext, &store).unwrap();
        assert_eq!(&*unprotect_with(&protected, &store).unwrap(), plaintext);
        assert!(unprotect_with(&protected, &Store::default()).is_err());
        let wrapping_key = store.0.lock().unwrap().values().next().unwrap().clone();
        assert!(!protected.windows(32).any(|bytes| bytes == wrapping_key));
        assert!(
            !protected
                .windows(plaintext.len())
                .any(|bytes| bytes == plaintext)
        );
        for index in 0..protected.len() {
            let mut corrupt = protected.clone();
            corrupt[index] ^= 1;
            assert!(unprotect_with(&corrupt, &store).is_err());
        }
        let (id, _, _) = split(&protected).unwrap();
        store.delete(id).unwrap();
        assert!(unprotect_with(&protected, &store).is_err());
    }
}
