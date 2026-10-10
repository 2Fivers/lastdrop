//! Start with Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Lastdrop`
//! (ARCHITECTURE "Autostart"). Per user, no elevation.
//!
//! Threading: main thread (settings window, first run); registry calls only, no locks held.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::path::Path;

/// The Run key under `HKEY_CURRENT_USER`.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value name.
pub const VALUE_NAME: &str = "Lastdrop";

/// The command-line flag the Run entry passes, so a start by Windows can skip opening anything.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Whether the Run value exists and points at `exe`.
pub fn is_enabled(exe: &Path) -> std::io::Result<bool> {
    todo!("M1: read the Run value")
}

/// Writes `"<exe>" --autostart` to the Run value.
pub fn enable(exe: &Path) -> std::io::Result<()> {
    todo!("M1: write the Run value")
}

/// Deletes the Run value (not an error when it does not exist).
pub fn disable() -> std::io::Result<()> {
    todo!("M1: delete the Run value")
}
