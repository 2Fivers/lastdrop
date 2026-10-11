//! Handing things to the shell on the user's request: a service's usage page in the default
//! browser (F§9 "Enter on a card"), and the vendor's own sign-in in a visible console window
//! (F§4.3 `Open sign-in…`). Lastdrop never reads what either does.
//!
//! Threading: main thread, from a flyout action (which then hides the flyout, F§1.4).

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::CREATE_NEW_CONSOLE;
use windows::Win32::UI::Shell::{
    FOLDERID_Profile, FOLDERID_System, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::GUID;
use windows::core::{Error, PCWSTR, w};

/// Longest URL handed to the shell.
const MAX_URL_LEN: usize = 2048;

/// Opens `url` in the default browser (`ShellExecuteW` "open"). Only `https://` URLs from
/// Lastdrop's own provider table are accepted (`Provider::usage_page`); anything else is an
/// `E_INVALIDARG` error, so nothing user-supplied or read from another tool's files is launched.
///
/// "Well-formed" here is strict: lower-case `https://`, a host without user info, printable ASCII
/// only, none of `"`, `<`, `>`, `\`, `^`, `{`, `|`, `}` or a backtick, and no spaces.
pub fn open_url(url: &str) -> windows::core::Result<()> {
    if !is_allowed_url(url) {
        return Err(Error::new(
            E_INVALIDARG,
            "only plain https:// URLs are opened",
        ));
    }
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated strings that outlive the call; no window owner.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value greater than 32 on success, an SE_ERR_* code otherwise.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        let e = Error::from_thread();
        Err(if e.code().is_err() {
            e
        } else {
            Error::new(
                windows::Win32::Foundation::E_FAIL,
                format!("ShellExecuteW failed ({})", result.0 as usize),
            )
        })
    }
}

/// The URL check of [`open_url`].
fn is_allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    if url.len() > MAX_URL_LEN {
        return false;
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() || host.contains('@') {
        return false;
    }
    url.bytes()
        .all(|b| b.is_ascii_graphic() && !b"\"<>\\^`{|}".contains(&b))
}

/// Starts `program args` in a new console window (`CREATE_NEW_CONSOLE` through
/// `std::process::Command`, `cmd.exe /k` so the window stays open after the program exits), with
/// `env` applied (`Some` sets, `None` removes) and `cwd`. Does not wait. The input is a
/// `lastdrop_providers::io::TerminalCommand` taken apart by the app.
///
/// The command line is `cmd.exe /d /v:off /s /k ""program" "arg" …"` with System32's `cmd.exe`:
/// every token is quoted, so `&|<>^` stay literal; tokens containing `"`, `%` (which `cmd` would
/// expand even inside quotes) or control characters are refused with `InvalidInput`. Put paths
/// with such characters in `env` instead, which `cmd` never parses.
///
/// A bare `program` (`claude`) is looked up by `cmd`, which searches the current folder before
/// `PATH`. So the console never starts in Lastdrop's own folder (which may be Downloads, where a
/// planted `claude.cmd` would run): without `cwd` it starts in the user's profile folder, and
/// `NoDefaultCurrentDirectoryInExePath` keeps `cmd` from searching the current folder at all.
pub fn open_console(
    program: &OsStr,
    args: &[OsString],
    env: &[(OsString, Option<OsString>)],
    cwd: Option<&Path>,
) -> std::io::Result<()> {
    let line = console_line(program, args)?;
    let mut command = Command::new(system_cmd()?);
    command.raw_arg("/d /v:off /s /k").raw_arg(line);
    command.env("NoDefaultCurrentDirectoryInExePath", "1");
    for (name, value) in env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    match cwd {
        Some(cwd) => {
            command.current_dir(cwd);
        }
        None => {
            command.current_dir(known_folder(&FOLDERID_Profile)?);
        }
    }
    // A GUI process has no standard handles, so std leaves STARTF_USESTDHANDLES unset and the
    // child uses its new console.
    command.creation_flags(CREATE_NEW_CONSOLE.0);
    let child = command.spawn()?;
    // Not waited for: dropping the handle leaves the console running.
    drop(child);
    Ok(())
}

/// `""program" "arg" …"`: the argument of `cmd /s /k`, which strips the outer quotes.
fn console_line(program: &OsStr, args: &[OsString]) -> std::io::Result<OsString> {
    if program.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no program to start",
        ));
    }
    let mut line = OsString::from("\"");
    for (i, token) in std::iter::once(program)
        .chain(args.iter().map(OsString::as_os_str))
        .enumerate()
    {
        let bad = token
            .encode_wide()
            .any(|u| u == u16::from(b'"') || u == u16::from(b'%') || u < 0x20);
        if bad {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a console command token contains a quote, a percent sign or a control character",
            ));
        }
        if i > 0 {
            line.push(" ");
        }
        line.push("\"");
        line.push(token);
        line.push("\"");
    }
    line.push("\"");
    Ok(line)
}

/// `%SystemRoot%\System32\cmd.exe` from the known-folder API (never a `cmd.exe` found next to
/// Lastdrop or on `PATH`).
fn system_cmd() -> std::io::Result<PathBuf> {
    Ok(known_folder(&FOLDERID_System)?.join("cmd.exe"))
}

/// A known folder's path (`SHGetKnownFolderPath`).
fn known_folder(id: &GUID) -> std::io::Result<PathBuf> {
    // SAFETY: returns a CoTaskMem string that is freed below after copying.
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
        .map_err(std::io::Error::other)?;
    // SAFETY: `path` is a valid NUL-terminated string until CoTaskMemFree.
    let folder = unsafe {
        let s = path.to_string();
        CoTaskMemFree(Some(path.0 as *const _));
        s
    }
    .map_err(std::io::Error::other)?;
    Ok(PathBuf::from(folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_https_urls_pass() {
        for ok in [
            "https://claude.ai/settings/usage",
            "https://chatgpt.com/codex/settings/usage",
            "https://www.meshy.ai/settings/subscription?tab=credits#top",
            "https://example.com",
        ] {
            assert!(is_allowed_url(ok), "{ok}");
        }
        for bad in [
            "http://claude.ai/",
            "HTTPS://claude.ai/",
            "https://",
            "https:///path",
            "https://user@claude.ai/",
            "https://claude.ai/a b",
            "https://claude.ai/\"x",
            "https://claude.ai/<x>",
            "https://claude.ai/\\..\\x",
            "https://claude.ai/ö",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "C:\\Windows\\notepad.exe",
            "",
        ] {
            assert!(!is_allowed_url(bad), "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(!is_allowed_url(&long));
    }

    #[test]
    fn open_url_refuses_before_reaching_the_shell() {
        assert_eq!(
            open_url("file:///C:/x").map_err(|e| e.code()),
            Err(E_INVALIDARG)
        );
    }

    #[test]
    fn console_lines_quote_every_token() {
        let line = console_line(
            OsStr::new(r"C:\Program Files\nodejs\claude.cmd"),
            &[OsString::from("login"), OsString::from("a&b|c")],
        )
        .expect("line");
        assert_eq!(
            line,
            OsString::from(r#"""C:\Program Files\nodejs\claude.cmd" "login" "a&b|c"""#)
        );
        assert_eq!(
            console_line(OsStr::new("codex"), &[]).expect("line"),
            OsString::from(r#"""codex"""#)
        );
    }

    #[test]
    fn console_lines_refuse_quotes_percent_and_control_characters() {
        for bad in ["a\"b", "%USERPROFILE%", "line\nbreak"] {
            let err = console_line(OsStr::new("codex"), &[OsString::from(bad)]).expect_err(bad);
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        }
        assert!(console_line(OsStr::new(""), &[]).is_err());
        assert!(console_line(OsStr::new("co\"dex"), &[]).is_err());
    }

    #[test]
    fn the_console_starts_in_the_profile_folder() {
        let profile = known_folder(&FOLDERID_Profile).expect("profile");
        assert!(profile.is_absolute());
        assert!(profile.is_dir());
    }

    #[test]
    fn cmd_comes_from_system32() {
        let cmd = system_cmd().expect("System32");
        assert!(cmd.ends_with("cmd.exe"));
        assert!(cmd.is_absolute());
        assert!(
            cmd.parent()
                .and_then(Path::file_name)
                .is_some_and(|d| d.eq_ignore_ascii_case("System32"))
        );
    }
}
