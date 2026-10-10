//! A string that must never be printed: tokens, API keys, cookies (ARCHITECTURE "Never log a
//! secret").
//!
//! `Debug` and `Display` print `[redacted]`; the value is reachable only through
//! [`SecretString::expose`], so every place that uses a secret is greppable. Not serializable on
//! purpose: secrets live in Windows Credential Manager or in the other tool's own files, never in
//! Lastdrop's settings or logs.

use std::fmt;

/// A secret value held in memory only for as long as a fetch needs it.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret.
    pub fn new(value: String) -> SecretString {
        SecretString(value)
    }

    /// The secret itself. Call it only where the value leaves the process for its own service
    /// (an `Authorization` header) or goes to Credential Manager.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// `true` when the secret is the empty string (treated as "no credential").
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        // Best effort: overwrite the bytes before the allocation is freed. Not a guarantee
        // (copies may exist), only a smaller window. `black_box` keeps the compiler from
        // dropping the writes as dead stores right before the free.
        let len = self.0.len();
        self.0.clear();
        self.0.extend(std::iter::repeat_n('\0', len));
        std::hint::black_box(self.0.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::SecretString;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let s = SecretString::new("sk-test-not-a-real-key".to_owned());
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(s.expose(), "sk-test-not-a-real-key");
    }
}
