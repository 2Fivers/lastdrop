//! What installing Lastdrop for one Windows user writes besides the program file
//! (docs/design/updates-spec.md §2): a Start menu shortcut and the "Apps > Installed apps" entry
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lastdrop`); the processes that run
//! the program it replaces or removes, which it waits for ([`processes_running`]); and the
//! SHA-256 of a downloaded installer (BCrypt). Per user, no elevation.
//!
//! Threading: the installer's main thread, or a worker thread for the hash; no locks.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, HANDLE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash,
    BCryptFinishHash, BCryptHashData,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_YESNO,
    MessageBoxW,
};
use windows::core::{Interface, PCWSTR, PWSTR};

/// The uninstall entry's key under `HKEY_CURRENT_USER`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Lastdrop";

/// The publisher shown in "Installed apps".
pub const PUBLISHER: &str = "2Fivers";

fn wide(s: &OsStr) -> Vec<u16> {
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
    let set_text = |name: &str, value: &OsStr| -> std::io::Result<()> {
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
    let mut uninstall = OsString::from("\"");
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

/// A process that runs the program the installer replaces or removes: a handle to wait on
/// (`SYNCHRONIZE`), closed on drop.
#[derive(Debug)]
pub struct RunningProcess {
    handle: isize,
    pid: u32,
}

impl RunningProcess {
    /// Its process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Waits at most `wait` for the process to end: `Ok(true)` once it has, `Ok(false)` while it
    /// still runs.
    pub fn wait(&self, wait: Duration) -> std::io::Result<bool> {
        // `u32::MAX` would be INFINITE.
        let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: our open process handle, which has SYNCHRONIZE.
        let waited = unsafe { WaitForSingleObject(HANDLE(self.handle as *mut _), millis) };
        if waited == WAIT_OBJECT_0 {
            Ok(true)
        } else if waited == WAIT_TIMEOUT {
            Ok(false)
        } else if waited == WAIT_FAILED {
            Err(std::io::Error::last_os_error())
        } else {
            Err(std::io::Error::other(format!(
                "unexpected wait result {}",
                waited.0
            )))
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed exactly once.
        if let Err(e) = unsafe { CloseHandle(HANDLE(self.handle as *mut _)) } {
            log::warn!("install: closing a process handle failed: {e}");
        }
    }
}

/// Every process but this one whose program is the file `exe` (updates-spec §2): the processes of
/// a snapshot (Toolhelp) named like it, kept when their image (`QueryFullProcessImageNameW`) is
/// that file, both paths canonicalized. A process this one may not open (another user's, an
/// elevated one) is left out. Empty when `exe` does not exist.
pub fn processes_running(exe: &Path) -> std::io::Result<Vec<RunningProcess>> {
    let target = match std::fs::canonicalize(exe) {
        Ok(target) => target,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let Some(name) = target.file_name() else {
        return Ok(Vec::new());
    };
    let me = std::process::id();
    // SAFETY: a snapshot of the processes running now; closed below.
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(win32_io_error)?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // The longest path Windows has.
    let mut image = vec![0u16; 32 * 1024];
    let mut found = Vec::new();
    // SAFETY: an open snapshot and an entry whose size is set.
    let mut next = unsafe { Process32FirstW(snapshot, &mut entry) };
    let result = loop {
        match next {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_NO_MORE_FILES.to_hresult() => break Ok(found),
            Err(e) => break Err(win32_io_error(e)),
        }
        if entry.th32ProcessID != me
            && names_program(&entry.szExeFile, name)
            && let Some(process) = open_running(entry.th32ProcessID, &target, &mut image)
        {
            found.push(process);
        }
        // SAFETY: as above.
        next = unsafe { Process32NextW(snapshot, &mut entry) };
    };
    // SAFETY: the snapshot opened above, closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    result
}

/// Whether a snapshot entry's `szExeFile` (a NUL-terminated file name) is `name`, ignoring case.
fn names_program(exe_file: &[u16], name: &OsStr) -> bool {
    let len = exe_file
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(exe_file.len());
    String::from_utf16_lossy(&exe_file[..len]).to_lowercase()
        == name.to_string_lossy().to_lowercase()
}

/// Process `pid`, opened to wait for, when its image is `target` (canonical); `None` when it has
/// ended, cannot be opened or runs another program. `image` holds the image's path meanwhile.
fn open_running(pid: u32, target: &Path, image: &mut [u16]) -> Option<RunningProcess> {
    let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
    // SAFETY: plain call; the handle goes to the RunningProcess below, which closes it.
    let handle = match unsafe { OpenProcess(access, false, pid) } {
        Ok(handle) => handle,
        Err(e) => {
            log::info!("install: process {pid} not opened: {e}");
            return None;
        }
    };
    let process = RunningProcess {
        handle: handle.0 as isize,
        pid,
    };
    let mut len = u32::try_from(image.len()).unwrap_or(u32::MAX);
    // SAFETY: a handle with PROCESS_QUERY_LIMITED_INFORMATION; `image` holds `len` UTF-16 units.
    let named = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut len,
        )
    };
    if let Err(e) = named {
        log::info!("install: no program path for process {pid}: {e}");
        return None;
    }
    let path = PathBuf::from(OsString::from_wide(image.get(..len as usize)?));
    (std::fs::canonicalize(path).ok()? == target).then_some(process)
}

/// A `windows` error as an `io::Error` with the Win32 code when there is one.
fn win32_io_error(e: windows::core::Error) -> std::io::Error {
    match WIN32_ERROR::from_error(&e) {
        Some(code) => std::io::Error::from_raw_os_error(code.0 as i32),
        None => std::io::Error::other(e),
    }
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

    #[test]
    fn a_snapshot_entry_names_the_program_ignoring_case() {
        let utf16 = |s: &str| -> Vec<u16> { s.encode_utf16().collect() };
        let mut entry = [0u16; 260];
        for (slot, c) in entry.iter_mut().zip("LastDrop.EXE".encode_utf16()) {
            *slot = c;
        }
        let name = OsStr::new("lastdrop.exe");
        assert!(names_program(&entry, name));
        assert!(!names_program(&entry, OsStr::new("lastdrop.ex")));
        assert!(!names_program(&entry, OsStr::new("lastdrop.exe.old")));
        // Without a NUL the whole array is the name; after one, nothing more is.
        assert!(names_program(&utf16("lastdrop.exe"), name));
        assert!(names_program(
            &utf16("cmd.exe\0lastdrop.exe"),
            OsStr::new("cmd.exe")
        ));
        assert!(!names_program(&utf16("cmd.exe\0lastdrop.exe"), name));
        assert!(!names_program(&[0u16; 4], name));
    }

    fn system_program(folder: &str, file: &str) -> PathBuf {
        std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
            .join(folder)
            .join(file)
    }

    #[test]
    fn a_process_running_the_program_is_found_and_waited_for() {
        let cmd = system_program("System32", "cmd.exe");
        // `/k` with its input open: it waits for a command until it is killed, or until this
        // test process ends and the pipe closes.
        let mut child = std::process::Command::new(&cmd)
            .args(["/d", "/q", "/k"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("cmd.exe starts");
        // Other cmd.exe processes (a terminal) are found too: the test looks at its own only.
        let ours = processes_running(&cmd)
            .expect("listed")
            .into_iter()
            .find(|p| p.pid() == child.id())
            .expect("the child is found");
        // The same name in another folder is another program.
        let other = system_program("SysWOW64", "cmd.exe");
        assert!(
            processes_running(&other)
                .expect("listed")
                .iter()
                .all(|p| p.pid() != child.id())
        );
        assert!(!ours.wait(Duration::from_millis(50)).expect("waited"));
        child.kill().expect("killed");
        assert!(ours.wait(Duration::from_secs(10)).expect("waited"));
        let _ = child.wait();
    }

    #[test]
    fn this_process_and_a_missing_program_are_left_out() {
        let me = std::env::current_exe().expect("the test program");
        assert!(
            processes_running(&me)
                .expect("listed")
                .iter()
                .all(|p| p.pid() != std::process::id())
        );
        let missing = std::env::temp_dir().join(format!(
            "lastdrop-no-such-program-{}.exe",
            std::process::id()
        ));
        assert!(processes_running(&missing).expect("listed").is_empty());
    }
}
