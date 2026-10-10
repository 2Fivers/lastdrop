//! The production I/O through its public traits, without network access: `RealSpawner` with
//! `cmd.exe` children (no window, killed by the tests) and the parts of `UreqHttp` that need no
//! server. Loopback HTTP, job objects and line reading are unit-tested in `src/real.rs`.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::Duration;

use lastdrop_core::secret::SecretString;
use lastdrop_providers::io::{
    HeaderValue, Http, HttpError, HttpRequest, Method, SpawnSpec, Spawner,
};
use lastdrop_providers::real::{RealSpawner, UreqHttp};

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
