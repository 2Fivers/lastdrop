//! The system folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`), for the settings window's
//! **Add folder…** (accounts-spec §7). Dialog process only: it runs a modal loop on the calling
//! thread until the user picks a folder or cancels, which a dialog may do (ARCHITECTURE
//! "Processes").

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
    IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING, PCWSTR};

/// Shows the folder picker, owned by `owner` (modal to it), titled `title`, opening in `start`
/// when it exists. `Ok(None)` when the user cancels.
pub fn pick_folder(
    owner: Option<HWND>,
    title: &str,
    start: Option<&Path>,
) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: called on the dialog process's main thread; balanced below when it succeeded. The
    // thread may be in an apartment already (S_FALSE, still balanced); RPC_E_CHANGED_MODE leaves
    // it as it is.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    let result = pick(owner, title, start);
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    result
}

fn pick(
    owner: Option<HWND>,
    title: &str,
    start: Option<&Path>,
) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: plain COM calls on an object created here and released when it drops; every
    // string passed lives across the call it is passed to.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let options = dialog.GetOptions()?;
        dialog.SetOptions(
            options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR,
        )?;
        let title = HSTRING::from(title);
        dialog.SetTitle(&title)?;
        if let Some(start) = start.filter(|p| p.is_dir()) {
            let wide: Vec<u16> = start.as_os_str().encode_wide().chain([0]).collect();
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&item);
            }
        }
        if let Err(e) = dialog.Show(owner) {
            if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(e);
        }
        let item = dialog.GetResult()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.as_ptr().cast()));
        Ok(Some(path))
    }
}
