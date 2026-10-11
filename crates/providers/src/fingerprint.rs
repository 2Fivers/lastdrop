//! Credential fingerprints of token files ([`Provider::credential_fingerprint`]), one format for
//! every provider: the scheduler stops a source after an auth error and lifts the stop when the
//! fingerprint changes (ARCHITECTURE: "stop on 401 until the credential file changes"), so a
//! missing file needs a fingerprint too, or signing in later would never be noticed.
//!
//! Only metadata is read, never the contents.
//!
//! [`Provider::credential_fingerprint`]: crate::Provider::credential_fingerprint

use std::io;
use std::path::Path;

use lastdrop_core::refresh::CredentialFingerprint;

use crate::FetchContext;

/// The fingerprint of a credential file that does not exist (signed out, never signed in).
pub const ABSENT: &str = "absent";

/// The fingerprint of the credential file at `path`: its name, size and modification time
/// (`auth.json:812:1791000000000000000`, `-` when the time is unknown); [`ABSENT`] when the file
/// does not exist or is a folder, so its appearance lifts an auth stop; `None` when the metadata
/// cannot be read (denied, locked): the provider cannot tell.
pub fn file_fingerprint(ctx: &FetchContext<'_>, path: &Path) -> Option<CredentialFingerprint> {
    match ctx.fs.metadata(path) {
        Ok(meta) if meta.is_dir => Some(absent()),
        Ok(meta) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            let modified = meta
                .modified
                .map_or_else(|| "-".to_owned(), |t| t.as_nanosecond().to_string());
            Some(CredentialFingerprint(format!(
                "{name}:{}:{modified}",
                meta.len
            )))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(absent()),
        Err(_) => None,
    }
}

/// The [`ABSENT`] fingerprint.
pub fn absent() -> CredentialFingerprint {
    CredentialFingerprint(ABSENT.to_owned())
}

/// Whether `fingerprint` is [`ABSENT`].
pub fn is_absent(fingerprint: &CredentialFingerprint) -> bool {
    fingerprint.0 == ABSENT
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jiff::{SignedDuration, Timestamp};

    use super::{absent, file_fingerprint, is_absent};
    use crate::fakes::Fakes;

    fn now() -> Timestamp {
        "2026-10-10T10:00:00Z".parse().expect("timestamp")
    }

    #[test]
    fn missing_files_and_folders_are_absent_and_files_name_size_and_time() {
        let f = Fakes::new(now());
        let ctx = f.context(SignedDuration::from_secs(30));
        let path = PathBuf::from(r"C:\Users\fixture\.tool\token.json");
        assert_eq!(file_fingerprint(&ctx, &path), Some(absent()));

        f.fs.add_file(&path, "secret-fixture", now());
        let first = file_fingerprint(&ctx, &path).expect("fingerprint");
        assert!(!is_absent(&first));
        assert!(first.0.starts_with("token.json:14:"), "{first:?}");
        assert!(!first.0.contains("secret"), "never the contents");
        assert!(!first.0.contains("fixture"), "never the folder");

        f.fs.add_file(
            &path,
            "secret-fixture",
            now() + SignedDuration::from_secs(1),
        );
        assert_ne!(file_fingerprint(&ctx, &path), Some(first));

        f.fs.remove(&path);
        f.fs.add_file(path.join("inside"), "x", now());
        assert_eq!(file_fingerprint(&ctx, &path), Some(absent()), "a folder");
    }
}
