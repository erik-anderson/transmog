use std::{io, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::{Zeroize, Zeroizing};

use crate::{envelope, invalid, payload, unavailable};

const BACKEND: u8 = 1;
// Domain separation, not a secret; DPAPI's user credentials provide protection.
const ENTROPY: &[u8] = b"Transmog interception CA private key v1";

fn transform(bytes: &[u8], encrypt: bool) -> io::Result<Zeroizing<Vec<u8>>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| invalid())?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(ENTROPY.len()).expect("small constant"),
        pbData: ENTROPY.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: All blobs and their byte buffers remain valid for the synchronous
    // call. DPAPI treats input/entropy as read-only. No prompt or description is
    // requested. Output is allocated by Windows and released with LocalFree.
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                ptr::from_ref(&input),
                ptr::null(),
                ptr::from_ref(&entropy),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                ptr::from_mut(&mut output),
            )
        } else {
            CryptUnprotectData(
                ptr::from_ref(&input),
                ptr::null_mut(),
                ptr::from_ref(&entropy),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                ptr::from_mut(&mut output),
            )
        }
    };
    if result == 0 {
        return Err(if encrypt {
            unavailable()
        } else {
            io::Error::other(
                "Windows DPAPI cannot decrypt the saved interception CA private key. The key is unlikely to be recoverable. Remove the old trusted root and set up a new interception certificate.",
            )
        });
    }
    if output.pbData.is_null() {
        return Err(invalid());
    }
    // SAFETY: On success DPAPI supplies cbData initialized bytes in its owned
    // allocation. Copy them, zero plaintext in place, and free exactly once.
    let bytes = unsafe {
        let data = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let result = Zeroizing::new(data.to_vec());
        data.zeroize();
        LocalFree(output.pbData.cast());
        result
    };
    Ok(bytes)
}
pub(super) fn protect(bytes: &[u8]) -> io::Result<Vec<u8>> {
    Ok(envelope(BACKEND, &transform(bytes, true)?))
}
pub(super) fn unprotect(bytes: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
    transform(payload(bytes, BACKEND)?, false)
}
pub(super) fn forget(bytes: &[u8]) -> io::Result<()> {
    payload(bytes, BACKEND).map(|_| ())
}
