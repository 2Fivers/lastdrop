//! What installing Lastdrop for one Windows user writes besides the program file
//! (docs/design/updates-spec.md §2): a Start menu shortcut and the "Apps > Installed apps" entry
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lastdrop`); and the SHA-256 of a
//! downloaded installer (BCrypt). Per user, no elevation.
//!
//! Threading: the installer's main thread, or a worker thread for the hash; no locks.

use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::Security::Cryptography::{
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash,
    BCryptFinishHash, BCryptHashData,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_YESNO,
    MessageBoxW,
};
use windows::core::{Interface, PCWSTR};

/// The uninstall entry's key under `HKEY_CURRENT_USER`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Lastdrop";

/// The publisher shown in "Installed apps".
pub const PUBLISHER: &str = "2Fivers";

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn check(status: WIN32_ERROR) -> std::io::Result<()> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status.0 as i32))
    }
}

/// The entry of "Apps > Installed apps".
pub struct UninstallEntry<'a> {
    /// `DisplayName`.
    pub name: &'a str,
    /// `DisplayVersion`.
    pub version: &'a str,
    /// The installed program: `DisplayIcon`, `UninstallString` as `"<exe>" --uninstall` and
    /// `QuietUninstallString` (what winget runs) as `"<exe>" --uninstall --quiet`.
    pub exe: &'a Path,
    /// `InstallLocation`.
    pub folder: &'a Path,
    /// `EstimatedSize`, in KB.
    pub size_kb: u32,
}

/// Writes the uninstall entry (replacing it).
pub fn write_uninstall_entry(entry: &UninstallEntry<'_>) -> std::io::Result<()> {
    let key_name = wide(UNINSTALL_KEY.as_ref());
    let mut key = HKEY::default();
    // SAFETY: valid NUL-terminated name and out pointer.
    check(unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    })?;
    let set_text = |name: &str, value: &std::ffi::OsStr| -> std::io::Result<()> {
        let name = wide(name.as_ref());
        let value = wide(value);
        let bytes: Vec<u8> = value.iter().flat_map(|u| u.to_le_bytes()).collect();
        // SAFETY: an open key; the data is a NUL-terminated UTF-16 string of `bytes.len()` bytes.
        check(unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) })
    };
    let set_dword = |name: &str, value: u32| -> std::io::Result<()> {
        let name = wide(name.as_ref());
        // SAFETY: as above, four bytes of a DWORD.
        check(unsafe {
            RegSetValueExW(
                key,
                PCWSTR(name.as_ptr()),
                None,
                REG_DWORD,
                Some(&value.to_le_bytes()),
            )
        })
    };
    let mut uninstall = std::ffi::OsString::from("\"");
    uninstall.push(entry.exe.as_os_str());
    uninstall.push("\" --uninstall");
    let mut quiet_uninstall = uninstall.clone();
    quiet_uninstall.push(" --quiet");
    let result = (|| {
        set_text("DisplayName", entry.name.as_ref())?;
        set_text("DisplayVersion", entry.version.as_ref())?;
        set_text("Publisher", PUBLISHER.as_ref())?;
        set_text("DisplayIcon", entry.exe.as_os_str())?;
        set_text("InstallLocation", entry.folder.as_os_str())?;
        set_text("UninstallString", &uninstall)?;
        set_text("QuietUninstallString", &quiet_uninstall)?;
        set_dword("NoModify", 1)?;
        set_dword("NoRepair", 1)?;
        set_dword("EstimatedSize", entry.size_kb)
    })();
    // SAFETY: the key opened above.
    let _ = unsafe { RegCloseKey(key) };
    result
}

/// Deletes the uninstall entry (not an error when it does not exist).
pub fn remove_uninstall_entry() -> std::io::Result<()> {
    let key_name = wide(UNINSTALL_KEY.as_ref());
    // SAFETY: a valid NUL-terminated subkey name under HKCU.
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status)
}

/// The user's Start menu "Programs" folder: `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
pub fn start_menu_programs() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs"),
    )
}

/// Writes a shortcut at `link` (a `.lnk` file) to `target`, with `description`.
pub fn write_shortcut(link: &Path, target: &Path, description: &str) -> windows::core::Result<()> {
    // SAFETY: COM for this call on this thread; balanced below. An already initialized thread
    // (S_FALSE, or another apartment) still lets CoCreateInstance work.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let result = (|| {
        // SAFETY: creating the shell's link object; plain COM calls with NUL-terminated strings
        // that outlive them.
        unsafe {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let target_w = wide(target.as_os_str());
            shell_link.SetPath(PCWSTR(target_w.as_ptr()))?;
            if let Some(dir) = target.parent() {
                let dir_w = wide(dir.as_os_str());
                shell_link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()))?;
            }
            let desc_w = wide(description.as_ref());
            shell_link.SetDescription(PCWSTR(desc_w.as_ptr()))?;
            let file: IPersistFile = shell_link.cast()?;
            let link_w = wide(link.as_os_str());
            file.Save(PCWSTR(link_w.as_ptr()), true)
        }
    })();
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    result
}

/// A modal message box (`MessageBoxW`) with Yes and No; `true` for Yes. The installer's only UI
/// (updates-spec §2): it runs before any Slint window exists.
pub fn ask(title: &str, text: &str) -> bool {
    let (title, text) = (wide(title.as_ref()), wide(text.as_ref()));
    // SAFETY: NUL-terminated strings that outlive the call; no owner window.
    let answer = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND,
        )
    };
    answer == IDYES
}

/// A modal message box with OK, as information or as an error.
pub fn tell(title: &str, text: &str, error: bool) {
    let (title, text) = (wide(title.as_ref()), wide(text.as_ref()));
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    // SAFETY: as in [`ask`].
    let _ = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | icon | MB_SETFOREGROUND,
        )
    };
}

/// The SHA-256 of the file at `path` (BCrypt), read in 64 KB pieces.
pub fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = BCRYPT_HASH_HANDLE::default();
    let to_io = |e: windows::core::Error| std::io::Error::other(e.to_string());
    // SAFETY: the SHA-256 pseudo-handle needs no provider; `hash` is a valid out pointer, and
    // BCrypt allocates the hash object itself when none is given.
    unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut hash, None, None, 0) }
        .ok()
        .map_err(to_io)?;
    let result = (|| {
        let mut buf = vec![0u8; 64 << 10];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            // SAFETY: an open hash handle; the slice is valid for the call.
            unsafe { BCryptHashData(hash, &buf[..n], 0) }
                .ok()
                .map_err(to_io)?;
        }
        let mut digest = [0u8; 32];
        // SAFETY: as above; SHA-256 writes exactly 32 bytes.
        unsafe { BCryptFinishHash(hash, &mut digest, 0) }
            .ok()
            .map_err(to_io)?;
        Ok(digest)
    })();
    // SAFETY: the handle created above.
    let _ = unsafe { BCryptDestroyHash(hash) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_known_input() {
        let path =
            std::env::temp_dir().join(format!("lastdrop-sha256-test-{}.txt", std::process::id()));
        std::fs::write(&path, b"abc").expect("write");
        let digest = sha256_file(&path).expect("hash");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }
}
