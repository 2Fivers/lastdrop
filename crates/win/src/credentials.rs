//! Lastdrop's own secrets in Windows Credential Manager: generic credentials (`CRED_TYPE_GENERIC`,
//! `CRED_PERSIST_LOCAL_MACHINE`) named `Lastdrop:<provider>:<account>:<name>` (ARCHITECTURE
//! "Secrets"). Never another tool's credentials. The app adapts this module to
//! `lastdrop_providers::io::Secrets`.
//!
//! Threading: any thread (refresh workers read, the settings window writes).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use lastdrop_core::secret::SecretString;

/// A Credential Manager failure (Win32 error text; never the secret).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Credential Manager: {0}")]
pub struct CredentialError(pub String);

/// Reads the secret of `target` (`CredReadW`); `None` when it does not exist
/// (`ERROR_NOT_FOUND`). The blob is UTF-16 as written by [`write()`].
pub fn read(target: &str) -> Result<Option<SecretString>, CredentialError> {
    todo!("M1: CredReadW")
}

/// Creates or replaces the secret of `target` (`CredWriteW`, user name `Lastdrop`).
pub fn write(target: &str, secret: &SecretString) -> Result<(), CredentialError> {
    todo!("M1: CredWriteW")
}

/// Deletes `target` (`CredDeleteW`); not an error when it does not exist.
pub fn delete(target: &str) -> Result<(), CredentialError> {
    todo!("M1: CredDeleteW")
}

/// `LastWritten` of `target` as a FILETIME value, for the credential fingerprint
/// (`lastdrop_core::refresh::CredentialFingerprint`); `None` when it does not exist.
pub fn last_written(target: &str) -> Result<Option<u64>, CredentialError> {
    todo!("M1: CredReadW LastWritten")
}
