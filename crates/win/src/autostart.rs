//! Start with Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Lastdrop`
//! (ARCHITECTURE "Autostart"). Per user, no elevation.
//!
//! Threading: the dialog process's main thread (the settings window's Apply); registry calls
//! only, no locks held.

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
};
use windows::core::PCWSTR;

/// The Run key under `HKEY_CURRENT_USER`.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value name.
pub const VALUE_NAME: &str = "Lastdrop";

/// The command-line flag the Run entry passes, so a start by Windows can skip opening anything.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Whether the Run value exists and points at `exe` (the first token of the command, compared
/// case-insensitively like Windows paths).
pub fn is_enabled(exe: &Path) -> std::io::Result<bool> {
    is_enabled_at(RUN_KEY, VALUE_NAME, exe)
}

/// Writes `"<exe>" --autostart` to the Run value.
pub fn enable(exe: &Path) -> std::io::Result<()> {
    enable_at(RUN_KEY, VALUE_NAME, exe)
}

/// Whether the Run value exists, whatever program it names (the installer moves it to the
/// installed copy, updates-spec §2).
pub fn is_set() -> std::io::Result<bool> {
    Ok(read_string(RUN_KEY, VALUE_NAME)?.is_some())
}

/// Deletes the Run value (not an error when it does not exist).
pub fn disable() -> std::io::Result<()> {
    disable_at(RUN_KEY, VALUE_NAME)
}

/// The command written to the Run value.
fn command_for(exe: &Path) -> OsString {
    let mut command = OsString::from("\"");
    command.push(exe.as_os_str());
    command.push("\" ");
    command.push(AUTOSTART_ARG);
    command
}

/// The program of a Run command: the quoted first token, or everything up to the first space.
fn program_of(command: &[u16]) -> &[u16] {
    let quote = u16::from(b'"');
    let space = u16::from(b' ');
    match command.split_first() {
        Some((&first, rest)) if first == quote => {
            let end = rest.iter().position(|&u| u == quote).unwrap_or(rest.len());
            &rest[..end]
        }
        _ => {
            let end = command
                .iter()
                .position(|&u| u == space)
                .unwrap_or(command.len());
            &command[..end]
        }
    }
}

/// Paths compare case-insensitively on Windows (NTFS default); `/` and `\` are the same.
fn same_path(a: &[u16], b: &[u16]) -> bool {
    let norm = |units: &[u16]| -> String {
        String::from_utf16_lossy(units)
            .replace('/', "\\")
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn io_error(code: WIN32_ERROR) -> std::io::Error {
    std::io::Error::from_raw_os_error(code.0 as i32)
}

fn is_enabled_at(key: &str, value: &str, exe: &Path) -> std::io::Result<bool> {
    let Some(command) = read_string(key, value)? else {
        return Ok(false);
    };
    let exe: Vec<u16> = exe.as_os_str().encode_wide().collect();
    Ok(same_path(program_of(&command), &exe))
}

/// A REG_SZ value as UTF-16 without the terminator; `None` when the key or value is missing.
fn read_string(key: &str, value: &str) -> std::io::Result<Option<Vec<u16>>> {
    let (key, value) = (wide(key), wide(value));
    let mut size: u32 = 0;
    // A value can change between the size query and the read; retry a few times.
    for _ in 0..4 {
        // SAFETY: size query (no data pointer) with valid NUL-terminated names.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        status.ok().map_err(|_| io_error(status))?;
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let mut written = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` has `written` bytes of writable space; RRF_RT_REG_SZ guarantees a
        // NUL-terminated string within it on success.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut written),
            )
        };
        match status {
            s if s.is_ok() => {
                buffer.truncate(written as usize / 2);
                while buffer.last() == Some(&0) {
                    buffer.pop();
                }
                return Ok(Some(buffer));
            }
            s if s == ERROR_FILE_NOT_FOUND => return Ok(None),
            s if s == windows::Win32::Foundation::ERROR_MORE_DATA => size = written,
            s => return Err(io_error(s)),
        }
    }
    Err(std::io::Error::other(
        "the Run value kept changing while it was read",
    ))
}

fn enable_at(key: &str, value: &str, exe: &Path) -> std::io::Result<()> {
    let command = command_for(exe);
    let data: Vec<u8> = command
        .encode_wide()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let (key, value) = (wide(key), wide(value));
    let mut hkey = HKEY::default();
    // SAFETY: creates or opens a key under HKCU for writing; `hkey` receives the handle, which is
    // closed below.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        )
    };
    status.ok().map_err(|_| io_error(status))?;
    // SAFETY: `hkey` is open for KEY_SET_VALUE; `data` is a NUL-terminated UTF-16 string.
    let status = unsafe { RegSetValueExW(hkey, PCWSTR(value.as_ptr()), None, REG_SZ, Some(&data)) };
    // SAFETY: closing the handle opened above.
    let _ = unsafe { RegCloseKey(hkey) };
    status.ok().map_err(|_| io_error(status))
}

fn disable_at(key: &str, value: &str) -> std::io::Result<()> {
    let (key, value) = (wide(key), wide(value));
    // SAFETY: valid NUL-terminated names under HKCU.
    let status = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
        )
    };
    if status.is_ok() || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(io_error(status))
    }
}

/// For tests: the command as a string.
#[cfg(test)]
fn command_string(exe: &Path) -> String {
    use std::os::windows::ffi::OsStringExt;
    let units: Vec<u16> = command_for(exe).encode_wide().collect();
    OsString::from_wide(&units).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use windows::Win32::System::Registry::RegDeleteTreeW;

    use super::*;

    /// A throwaway key under HKCU, deleted (with its values) when dropped. Never the real Run key.
    struct ScratchKey(String);

    impl ScratchKey {
        fn new(tag: &str) -> ScratchKey {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            ScratchKey(format!(
                r"Software\Lastdrop.Tests\{tag}-{}-{nanos}",
                std::process::id()
            ))
        }
    }

    impl Drop for ScratchKey {
        fn drop(&mut self) {
            let key = wide(&self.0);
            // SAFETY: deletes only the scratch key this test created.
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
            let parent = wide(r"Software\Lastdrop.Tests");
            // SAFETY: removes the (now possibly empty) scratch parent; fails harmlessly while
            // another test still has a key under it.
            let _ = unsafe {
                windows::Win32::System::Registry::RegDeleteKeyW(
                    HKEY_CURRENT_USER,
                    PCWSTR(parent.as_ptr()),
                )
            };
        }
    }

    #[test]
    fn the_command_quotes_the_exe_and_passes_the_flag() {
        let exe = PathBuf::from(r"C:\Program Files\Lastdrop\lastdrop.exe");
        assert_eq!(
            command_string(&exe),
            r#""C:\Program Files\Lastdrop\lastdrop.exe" --autostart"#
        );
    }

    #[test]
    fn program_is_the_first_token() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(
            program_of(&w(r#""C:\a b\x.exe" --autostart"#)),
            &w(r"C:\a b\x.exe")[..]
        );
        assert_eq!(
            program_of(&w(r"C:\ab\x.exe --autostart")),
            &w(r"C:\ab\x.exe")[..]
        );
        assert_eq!(program_of(&w(r#""unterminated"#)), &w("unterminated")[..]);
        assert!(same_path(
            &w(r"C:\Apps\LastDrop.EXE"),
            &w(r"c:/apps/lastdrop.exe")
        ));
        assert!(!same_path(
            &w(r"C:\Apps\lastdrop.exe"),
            &w(r"C:\Other\lastdrop.exe")
        ));
    }

    #[test]
    fn enable_check_and_disable_round_trip_in_a_scratch_key() {
        let key = ScratchKey::new("autostart");
        let exe = PathBuf::from(r"C:\Users\Example\AppData\Local\Lastdrop\lastdrop.exe");
        let other = PathBuf::from(r"D:\Elsewhere\lastdrop.exe");

        assert!(!is_enabled_at(&key.0, VALUE_NAME, &exe).expect("read missing key"));
        disable_at(&key.0, VALUE_NAME).expect("deleting a missing value is fine");

        enable_at(&key.0, VALUE_NAME, &exe).expect("enable");
        assert!(is_enabled_at(&key.0, VALUE_NAME, &exe).expect("read"));
        assert!(!is_enabled_at(&key.0, VALUE_NAME, &other).expect("read"));
        let stored = read_string(&key.0, VALUE_NAME)
            .expect("read")
            .expect("value");
        assert_eq!(String::from_utf16_lossy(&stored), command_string(&exe));

        // Re-enabling for another path replaces the value.
        enable_at(&key.0, VALUE_NAME, &other).expect("enable other");
        assert!(is_enabled_at(&key.0, VALUE_NAME, &other).expect("read"));
        assert!(!is_enabled_at(&key.0, VALUE_NAME, &exe).expect("read"));

        disable_at(&key.0, VALUE_NAME).expect("disable");
        assert!(!is_enabled_at(&key.0, VALUE_NAME, &other).expect("read"));
        disable_at(&key.0, VALUE_NAME).expect("disable twice");
    }
}
