//! Compiles the Slint UI with the fluent style, embeds the application manifest (per-monitor
//! v2 DPI awareness, Windows 10/11 supportedOS), as the spike does (ADR 0001), and links the app
//! icon (`lastdrop.rc`, compiled with the Windows SDK's `rc.exe`).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compiles");

    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest = Path::new(&dir).join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );

    let dir = Path::new(&dir);
    println!("cargo:rerun-if-changed=lastdrop.rc");
    println!("cargo:rerun-if-changed=icons/lastdrop.ico");
    println!("cargo:rerun-if-env-changed=LASTDROP_RC");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("lastdrop.res");
    let rc = rc_exe();
    let status = Command::new(&rc)
        .current_dir(dir)
        .args(["/nologo", "/fo"])
        .arg(&out)
        .arg("lastdrop.rc")
        .status()
        .unwrap_or_else(|e| panic!("{} runs: {e}", rc.display()));
    assert!(status.success(), "rc.exe compiles lastdrop.rc: {status}");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// The resource compiler: `LASTDROP_RC` if set, else the newest x64 `rc.exe` of the Windows 10/11
/// SDK, which the MSVC toolchain needs anyway (its import libraries).
fn rc_exe() -> PathBuf {
    if let Some(rc) = std::env::var_os("LASTDROP_RC") {
        return PathBuf::from(rc);
    }
    let programs = std::env::var_os("ProgramFiles(x86)")
        .map_or_else(|| PathBuf::from(r"C:\Program Files (x86)"), PathBuf::from);
    let bin = programs.join(r"Windows Kits\10\bin");
    let newest = std::fs::read_dir(&bin)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join(r"x64\rc.exe"))
        .filter(|rc| rc.is_file())
        // Version folders (10.0.26100.0) sort correctly as version numbers, not as text.
        .max_by_key(|rc| sdk_version(rc));
    newest.unwrap_or_else(|| {
        panic!(
            "rc.exe not found under {}; install the Windows SDK or set LASTDROP_RC",
            bin.display()
        )
    })
}

/// The SDK version of `…\bin\<version>\x64\rc.exe` as numbers (`10.0.26100.0` → [10, 0, 26100, 0]).
fn sdk_version(rc: &Path) -> Vec<u32> {
    rc.ancestors()
        .nth(2)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(|n| n.split('.').map(|p| p.parse().unwrap_or(0)).collect())
        .unwrap_or_default()
}
