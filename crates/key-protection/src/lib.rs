#![deny(missing_docs, unsafe_op_in_unsafe_fn)]
//! Interception keys encrypted before touching disk. Windows uses current-user
//! DPAPI; macOS/Linux use AES-256-GCM with a per-file key in Keychain/Secret Service.
//! OS protection errors never fall back to plaintext or replace an existing CA.

use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod keyring;
#[cfg(windows)]
mod windows;

const MAGIC: &[u8] = b"TRANSMOG-CA-KEY\0";
const VERSION: u8 = 1;
const MAX_BYTES: u64 = 1024 * 1024;

#[cfg(test)]
mod tests;

/// Injectable secret-protection boundary. Errors must not include secret bytes.
pub trait KeyProtection: Send + Sync {
    /// Wraps plaintext with user-bound OS protection.
    /// # Errors
    /// Returns an error when OS protection is unavailable.
    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>>;
    /// Unwraps material produced by this protector.
    /// # Errors
    /// Returns an error for corruption, inaccessible credentials or another user.
    fn unprotect(&self, protected: &[u8]) -> io::Result<Zeroizing<Vec<u8>>>;
    /// Deletes any external wrapping secret. Missing secrets are already removed.
    /// # Errors
    /// Returns an error when secret cleanup fails.
    fn forget(&self, protected: &[u8]) -> io::Result<()>;
}

/// Production protector using the current user's platform credential facilities.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemKeyProtection;

impl KeyProtection for SystemKeyProtection {
    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>> {
        platform_protect(plaintext)
    }
    fn unprotect(&self, protected: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        platform_unprotect(protected)
    }
    fn forget(&self, protected: &[u8]) -> io::Result<()> {
        platform_forget(protected)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
use keyring::{
    forget as platform_forget, protect as platform_protect, unprotect as platform_unprotect,
};
#[cfg(windows)]
use windows::{
    forget as platform_forget, protect as platform_protect, unprotect as platform_unprotect,
};

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn platform_protect(_: &[u8]) -> io::Result<Vec<u8>> {
    Err(unavailable())
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn platform_unprotect(_: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
    Err(unavailable())
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn platform_forget(_: &[u8]) -> io::Result<()> {
    Err(unavailable())
}

fn unavailable() -> io::Error {
    io::Error::other(
        "Interception CA key protection is unavailable. Use the original OS user account and unlock its credential store; on Linux a Secret Service is required.",
    )
}
fn invalid() -> io::Error {
    io::Error::other(
        "Interception CA key protection data is invalid or belongs to another platform",
    )
}

fn envelope(backend: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&[VERSION, backend]);
    bytes.extend_from_slice(payload);
    bytes
}
fn payload(bytes: &[u8], backend: u8) -> io::Result<&[u8]> {
    let data = bytes.strip_prefix(MAGIC).ok_or_else(invalid)?;
    if data.get(..2) != Some(&[VERSION, backend]) || data.len() <= 2 {
        return Err(invalid());
    }
    Ok(&data[2..])
}

/// Whether file bytes use the protected format, including unsupported versions.
pub fn is_protected(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn is_legacy_pem(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    [
        b"-----BEGIN PRIVATE KEY-----".as_slice(),
        b"-----BEGIN RSA PRIVATE KEY-----",
        b"-----BEGIN EC PRIVATE KEY-----",
    ]
    .iter()
    .any(|marker| bytes.starts_with(marker))
}

fn read_bytes(path: &Path) -> io::Result<Zeroizing<Vec<u8>>> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::other(
            "Interception CA key must be a regular file",
        ));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}

/// Reads protected keys and legacy PEM inputs without modifying the file.
/// # Errors
/// Returns a bounded I/O or credential-store failure; protected data never falls
/// back to PEM if decryption fails.
pub fn read(path: &Path, protection: &dyn KeyProtection) -> io::Result<Zeroizing<Vec<u8>>> {
    let bytes = read_bytes(path)?;
    if is_protected(&bytes) {
        protection.unprotect(&bytes)
    } else if is_legacy_pem(&bytes) {
        Ok(bytes)
    } else {
        Err(invalid())
    }
}

fn publish(path: &Path, bytes: &[u8], replace: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // NamedTempFile is created with mode 0600 on Unix. Only ciphertext is written.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path)
    } else {
        temporary.persist_noclobber(path)
    }
    .map_err(|error| error.error)?;
    Ok(())
}

/// Creates a protected key file without overwriting an existing destination.
/// # Errors
/// Protection and publication failures leave no plaintext file behind.
pub fn write_new(path: &Path, plaintext: &[u8], protection: &dyn KeyProtection) -> io::Result<()> {
    let bytes = protection.protect(plaintext)?;
    if let Err(error) = publish(path, &bytes, false) {
        let _ = protection.forget(&bytes);
        return Err(error);
    }
    Ok(())
}

/// Atomically protects a legacy file after the caller validates its CA identity.
/// Already protected files are verified, never rewrapped or silently replaced.
/// # Errors
/// Returns a protection or I/O error, retaining the original file on failure.
pub fn protect_existing(path: &Path, protection: &dyn KeyProtection) -> io::Result<()> {
    let bytes = read_bytes(path)?;
    if is_protected(&bytes) {
        protection.unprotect(&bytes)?;
        return Ok(());
    }
    if !is_legacy_pem(&bytes) {
        return Err(invalid());
    }
    let protected = protection.protect(&bytes)?;
    // Detect ordinary concurrent edits before replacement.
    let current = match read_bytes(path) {
        Ok(current) => current,
        Err(error) => {
            let _ = protection.forget(&protected);
            return Err(error);
        }
    };
    if *current != *bytes {
        let _ = protection.forget(&protected);
        return Err(io::Error::other(
            "Interception CA key changed during protection",
        ));
    }
    if let Err(error) = publish(path, &protected, true) {
        let _ = protection.forget(&protected);
        return Err(error);
    }
    Ok(())
}

/// Removes a key and its external wrapping secret. Missing files are harmless.
/// # Errors
/// Returns an I/O or secret-cleanup failure, retaining the file for cleanup retry.
pub fn remove(path: &Path, protection: &dyn KeyProtection) -> io::Result<()> {
    let bytes = match read_bytes(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    if is_protected(&bytes) {
        protection.forget(&bytes)?;
    }
    std::fs::remove_file(path)
}
