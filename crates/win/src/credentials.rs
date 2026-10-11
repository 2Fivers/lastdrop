//! Lastdrop's own secrets in Windows Credential Manager: generic credentials (`CRED_TYPE_GENERIC`,
//! `CRED_PERSIST_LOCAL_MACHINE`) named `Lastdrop:<provider>:<account>:<name>` (ARCHITECTURE
//! "Secrets"). Never another tool's credentials. The app adapts this module to
//! `lastdrop_providers::io::Secrets`.
//!
//! The blob is the secret's UTF-16 code units (little endian, no terminator). Every buffer that
//! held the secret on our side is overwritten before it is freed (best effort, as
//! [`SecretString`]'s own drop). Error texts carry the Win32 message only — never the secret and
//! never the target name (it contains the account label).
//!
//! Threading: any thread (refresh workers read, the settings window writes).

use lastdrop_core::secret::SecretString;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME, WIN32_ERROR};
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};

/// A Credential Manager failure (Win32 error text; never the secret).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Credential Manager: {0}")]
pub struct CredentialError(pub String);

/// The user name stored with every Lastdrop credential.
const USER_NAME: &str = "Lastdrop";

/// Reads the secret of `target` (`CredReadW`); `None` when it does not exist
/// (`ERROR_NOT_FOUND`). The blob is UTF-16 as written by [`write()`].
pub fn read(target: &str) -> Result<Option<SecretString>, CredentialError> {
    let Some(cred) = Credential::read(target)? else {
        return Ok(None);
    };
    cred.secret().map(Some)
}

/// Creates or replaces the secret of `target` (`CredWriteW`, user name `Lastdrop`).
pub fn write(target: &str, secret: &SecretString) -> Result<(), CredentialError> {
    let mut target = Wide::new(target)?;
    let mut user = Wide::new(USER_NAME)?;
    let mut blob = Zeroizing(
        secret
            .expose()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<u8>>(),
    );
    if blob.0.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(CredentialError(format!(
            "the secret is longer than Credential Manager allows ({} bytes)",
            CRED_MAX_CREDENTIAL_BLOB_SIZE
        )));
    }
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.0.as_mut_ptr()),
        CredentialBlobSize: blob.0.len() as u32,
        CredentialBlob: blob.0.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.0.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` points into a live, NUL-terminated (strings) or
    // `CredentialBlobSize`-long (blob) buffer owned by this frame; CredWriteW only reads them.
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| win32_error("CredWriteW", &e))
}

/// Deletes `target` (`CredDeleteW`); not an error when it does not exist.
pub fn delete(target: &str) -> Result<(), CredentialError> {
    let target = Wide::new(target)?;
    // SAFETY: a NUL-terminated target name.
    match unsafe { CredDeleteW(PCWSTR(target.0.as_ptr()), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(win32_error("CredDeleteW", &e)),
    }
}

/// `LastWritten` of `target` as a FILETIME value, for the credential fingerprint
/// (`lastdrop_core::refresh::CredentialFingerprint`); `None` when it does not exist.
pub fn last_written(target: &str) -> Result<Option<u64>, CredentialError> {
    Ok(Credential::read(target)?.map(|cred| cred.last_written()))
}

/// A credential returned by `CredReadW`; the blob is zeroed and the memory freed on drop.
struct Credential(*mut CREDENTIALW);

impl Credential {
    fn read(target: &str) -> Result<Option<Credential>, CredentialError> {
        let target = Wide::new(target)?;
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: a NUL-terminated target name and an out-pointer that receives a buffer we free
        // with CredFree (in Drop).
        match unsafe {
            CredReadW(
                PCWSTR(target.0.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut cred,
            )
        } {
            Ok(()) if !cred.is_null() => Ok(Some(Credential(cred))),
            Ok(()) => Err(CredentialError(
                "CredReadW returned no credential".to_owned(),
            )),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(win32_error("CredReadW", &e)),
        }
    }

    fn blob(&self) -> &[u8] {
        // SAFETY: `self.0` is the live CREDENTIALW from CredReadW; its blob pointer and size
        // describe memory inside the same allocation (or are null/0).
        unsafe {
            let cred = &*self.0;
            if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
            }
        }
    }

    fn secret(&self) -> Result<SecretString, CredentialError> {
        let blob = self.blob();
        if !blob.len().is_multiple_of(2) {
            return Err(CredentialError(
                "the stored secret is not UTF-16 (odd length)".to_owned(),
            ));
        }
        let units = blob
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b));
        // Reserve the worst case (3 UTF-8 bytes per unit) so the String never reallocates and
        // leaves a partial copy of the secret behind in freed memory.
        let mut text = String::with_capacity(blob.len() / 2 * 3);
        for c in char::decode_utf16(units) {
            match c {
                Ok(c) => text.push(c),
                Err(_) => {
                    // Overwrite what was decoded so far before the String is freed.
                    let len = text.len();
                    text.clear();
                    text.extend(std::iter::repeat_n('\0', len));
                    std::hint::black_box(text.as_bytes());
                    return Err(CredentialError(
                        "the stored secret is not valid UTF-16".to_owned(),
                    ));
                }
            }
        }
        Ok(SecretString::new(text))
    }

    fn last_written(&self) -> u64 {
        // SAFETY: `self.0` is the live CREDENTIALW from CredReadW.
        let FILETIME {
            dwLowDateTime,
            dwHighDateTime,
        } = unsafe { (*self.0).LastWritten };
        (u64::from(dwHighDateTime) << 32) | u64::from(dwLowDateTime)
    }
}

impl Drop for Credential {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from CredReadW and is freed exactly once, here. The blob lies in
        // that writable allocation; it is overwritten first.
        unsafe {
            let cred = &*self.0;
            if !cred.CredentialBlob.is_null() {
                for i in 0..cred.CredentialBlobSize as usize {
                    std::ptr::write_volatile(cred.CredentialBlob.add(i), 0);
                }
            }
            CredFree(self.0.cast());
        }
    }
}

/// A byte buffer that is overwritten before it is freed.
struct Zeroizing(Vec<u8>);

impl Drop for Zeroizing {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // SAFETY: `b` is a valid, exclusive reference into the vector.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

/// A NUL-terminated UTF-16 string without interior NULs.
struct Wide(Vec<u16>);

impl Wide {
    fn new(s: &str) -> Result<Wide, CredentialError> {
        if s.contains('\0') {
            return Err(CredentialError(
                "a name contains a NUL character".to_owned(),
            ));
        }
        Ok(Wide(s.encode_utf16().chain(std::iter::once(0)).collect()))
    }
}

fn is_not_found(e: &windows::core::Error) -> bool {
    WIN32_ERROR::from_error(e) == Some(ERROR_NOT_FOUND)
}

fn win32_error(call: &str, e: &windows::core::Error) -> CredentialError {
    CredentialError(format!(
        "{call} failed: {} ({:#010x})",
        e.message(),
        e.code().0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway target, deleted when dropped (also when an assertion fails).
    struct Scratch(String);

    impl Scratch {
        fn new() -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            Scratch(format!(
                "Lastdrop:test:scratch-{}-{nanos}:e5",
                std::process::id()
            ))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete(&self.0);
        }
    }

    #[test]
    fn write_read_and_delete_a_throwaway_credential() {
        let target = Scratch::new();
        assert_eq!(read(&target.0), Ok(None));
        assert_eq!(last_written(&target.0), Ok(None));
        delete(&target.0).expect("deleting a missing credential is fine");

        // Invented test value with non-ASCII and an astral character.
        let secret = SecretString::new("invented-test-value-åäö-\u{1F4A7}".to_owned());
        write(&target.0, &secret).expect("write");
        let back = read(&target.0).expect("read").expect("present");
        assert_eq!(back, secret);
        let first = last_written(&target.0).expect("read").expect("present");
        assert!(first > 0);

        let replaced = SecretString::new("another-invented-value".to_owned());
        write(&target.0, &replaced).expect("replace");
        assert_eq!(read(&target.0).expect("read"), Some(replaced));
        assert!(last_written(&target.0).expect("read").expect("present") >= first);

        delete(&target.0).expect("delete");
        assert_eq!(read(&target.0), Ok(None));
        delete(&target.0).expect("delete twice");
    }

    #[test]
    fn rejects_oversized_secrets_and_nul_names() {
        let target = Scratch::new();
        let huge = SecretString::new("x".repeat(CRED_MAX_CREDENTIAL_BLOB_SIZE as usize));
        let err = write(&target.0, &huge).expect_err("too long");
        assert!(!err.0.contains('x'), "the error never repeats the secret");
        assert!(write("Lastdrop:a\0b", &SecretString::new("v".to_owned())).is_err());
        assert_eq!(read(&target.0), Ok(None));
    }

    #[test]
    fn errors_never_contain_the_target() {
        let e = win32_error(
            "CredReadW",
            &windows::core::Error::from_hresult(ERROR_NOT_FOUND.to_hresult()),
        );
        assert!(e.0.starts_with("CredReadW failed"));
        assert!(!e.0.contains("Lastdrop:"));
    }
}
