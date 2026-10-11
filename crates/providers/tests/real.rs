//! The production I/O through its public traits, without network access: `RealSpawner` with
//! `cmd.exe` children (no window, killed by the tests), `RealFs::open_at` on a file of its own in
//! the temp folder, and the parts of `UreqHttp` that need no server. Loopback HTTP, job objects
//! and line reading are unit-tested in `src/real.rs`.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;
use std::time::Duration;

use lastdrop_core::secret::SecretString;
use lastdrop_providers::io::{
    Fs, HeaderValue, Http, HttpError, HttpRequest, Method, SpawnSpec, Spawner,
};
use lastdrop_providers::real::{RealFs, RealSpawner, UreqHttp};

fn cmd_exe() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    PathBuf::from(root).join("System32").join("cmd.exe")
}

fn cmd(args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        program: cmd_exe(),
        args: args.iter().map(OsString::from).collect(),
        env: Vec::new(),
        cwd: None,
    }
}

/// Every line the child prints until end of stream; the child is killed afterwards either way.
fn all_lines(spec: &SpawnSpec) -> Vec<String> {
    let mut child = RealSpawner.spawn(spec).expect("spawn cmd.exe");
    let mut lines = Vec::new();
    while let Some(line) = child.read_line(Duration::from_secs(10)).expect("line") {
        lines.push(line);
    }
    child.kill().expect("kill");
    lines
}

#[test]
fn a_tiny_child_prints_lines_then_ends() {
    assert_eq!(
        all_lines(&cmd(&["/c", "echo one&echo two"])),
        vec!["one".to_owned(), "two".to_owned()]
    );
}

#[test]
fn a_missing_program_is_not_found() {
    let spec = SpawnSpec {
        program: std::env::temp_dir().join("lastdrop-no-such-program-4f1c.exe"),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
    };
    let err = RealSpawner.spawn(&spec).err().expect("missing program");
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn environment_overrides_set_and_remove() {
    let mut spec = cmd(&["/c", "echo [%LASTDROP_TEST_VALUE%][%OS%]"]);
    spec.env = vec![
        (
            OsString::from("LASTDROP_TEST_VALUE"),
            Some(OsString::from("set-by-test")),
        ),
        (OsString::from("OS"), None),
    ];
    // cmd leaves an undefined variable as written.
    assert_eq!(all_lines(&spec), vec!["[set-by-test][%OS%]".to_owned()]);
}

#[test]
fn the_working_directory_is_the_given_one() {
    let dir = std::env::temp_dir();
    let mut spec = cmd(&["/c", "cd"]);
    spec.cwd = Some(dir.clone());
    let lines = all_lines(&spec);
    let printed = lines.first().expect("cd prints the folder");
    let want = dir.to_string_lossy();
    assert!(
        printed
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(want.trim_end_matches('\\')),
        "{printed:?} vs {want:?}"
    );
}

#[test]
fn kill_is_idempotent_and_a_killed_child_reads_as_ended() {
    let mut child = RealSpawner
        .spawn(&cmd(&["/c", "set /p WAIT="]))
        .expect("spawn cmd.exe");
    child.kill().expect("kill");
    child.kill().expect("kill again");
    assert_eq!(
        child.read_line(Duration::from_millis(10)).expect("ended"),
        None
    );
    assert_eq!(
        child.write_line("late").expect_err("killed").kind(),
        ErrorKind::BrokenPipe
    );
}

#[test]
fn files_open_for_reads_at_any_offset_and_never_block_the_owner() {
    let dir = std::env::temp_dir().join(format!("lastdrop-open-at-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp folder");
    let path = dir.join("data_1");
    let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
    std::fs::write(&path, &data).expect("write");

    let mut file = RealFs.open_at(&path).expect("open");
    assert_eq!(file.size(), 10_000);
    let mut buf = [0u8; 300];
    file.read_exact_at(8192, &mut buf).expect("read");
    assert_eq!(&buf[..], &data[8192..8492]);
    file.read_exact_at(0, &mut buf[..4]).expect("read");
    assert_eq!(&buf[..4], &[0, 1, 2, 3]);
    assert_eq!(
        file.read_exact_at(9_900, &mut buf)
            .expect_err("past the end")
            .kind(),
        ErrorKind::UnexpectedEof
    );
    assert_eq!(
        file.read_exact_at(1 << 40, &mut buf)
            .expect_err("far past the end")
            .kind(),
        ErrorKind::UnexpectedEof
    );

    // While Lastdrop's handle is open the owner writes, renames and deletes (Chromium opens its
    // cache files with DELETE access).
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("another writer")
        .write_all(b"owner")
        .expect("write");
    let moved = dir.join("data_1.old");
    std::fs::rename(&path, &moved).expect("rename while open");
    std::fs::remove_file(&moved).expect("delete while open");
    drop(file);

    assert_eq!(
        RealFs.open_at(&path).err().expect("gone").kind(),
        ErrorKind::NotFound
    );
    std::fs::remove_dir(&dir).expect("clean up");
}

#[test]
fn secret_headers_never_print() {
    let request = HttpRequest {
        method: Method::Get,
        url: "https://api.example.invalid/v1/balance".to_owned(),
        headers: vec![(
            "Authorization".to_owned(),
            HeaderValue::Secret(SecretString::new("Bearer key-not-real".to_owned())),
        )],
        body: None,
        timeout: Duration::from_secs(5),
    };
    let text = format!("{request:?}");
    assert!(!text.contains("key-not-real"));
    assert!(text.contains("[redacted]"));
}

#[test]
fn the_client_refuses_plain_http_without_sending() {
    let http = UreqHttp::default();
    assert_eq!(format!("{http:?}"), "UreqHttp");
    let request = HttpRequest {
        method: Method::Get,
        url: "http://api.example.invalid/v1/balance".to_owned(),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(5),
    };
    assert!(matches!(http.request(&request), Err(HttpError::Other(_))));
}
