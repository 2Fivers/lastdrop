//! Handing things to the shell on the user's request: a service's usage page in the default
//! browser (F§9 "Enter on a card"), and the vendor's own sign-in in a visible console window
//! (F§4.3 `Open sign-in…`). Lastdrop never reads what either does.
//!
//! Threading: main thread, from a flyout action (which then hides the flyout, F§1.4).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Opens `url` in the default browser (`ShellExecuteW` "open"). Only `https://` URLs from
/// Lastdrop's own provider table are accepted (`Provider::usage_page`); anything else is an
/// `E_INVALIDARG` error, so nothing user-supplied or read from another tool's files is launched.
pub fn open_url(url: &str) -> windows::core::Result<()> {
    todo!("M1: ShellExecuteW open, https only")
}

/// Starts `program args` in a new console window (`CREATE_NEW_CONSOLE` through
/// `std::process::Command`, `cmd.exe /k` so the window stays open after the program exits), with
/// `env` applied (`Some` sets, `None` removes) and `cwd`. Does not wait. The input is a
/// `lastdrop_providers::io::TerminalCommand` taken apart by the app.
pub fn open_console(
    program: &OsStr,
    args: &[OsString],
    env: &[(OsString, Option<OsString>)],
    cwd: Option<&Path>,
) -> std::io::Result<()> {
    todo!("M1: console with CREATE_NEW_CONSOLE")
}
