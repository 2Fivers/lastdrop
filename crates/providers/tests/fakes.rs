//! The fakes behave as the provider tests expect. All data is invented.

use std::io::{ErrorKind, Read};
use std::path::Path;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, ProviderId};
use lastdrop_core::secret::SecretString;
use lastdrop_providers::fakes::{FakeLine, Fakes};
use lastdrop_providers::io::{
    Clock, CredentialName, Env, Fs, HeaderValue, Http, HttpError, HttpRequest, HttpResponse,
    Method, Secrets, SpawnSpec, Spawner,
};

fn t0() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

#[test]
fn clock_moves_only_when_told_and_the_context_has_a_deadline() {
    let f = Fakes::new(t0());
    let ctx = f.context(SignedDuration::from_secs(30));
    assert_eq!(ctx.remaining(), Duration::from_secs(30));
    f.clock.advance(SignedDuration::from_secs(10));
    assert_eq!(f.clock.now(), t0() + SignedDuration::from_secs(10));
    assert_eq!(ctx.remaining(), Duration::from_secs(20));
    f.clock.advance(SignedDuration::from_secs(60));
    assert_eq!(ctx.remaining(), Duration::ZERO);
}

#[test]
fn http_answers_from_the_script_and_records_requests_with_secrets_redacted() {
    let f = Fakes::new(t0());
    let url = "https://api.example.invalid/v1/balance";
    f.http.respond(
        Method::Get,
        url,
        HttpResponse::new(200, r#"{"balance": 10}"#),
    );
    let request = HttpRequest {
        method: Method::Get,
        url: url.to_owned(),
        headers: vec![(
            "Authorization".to_owned(),
            HeaderValue::Secret(SecretString::new("Bearer test-token-not-real".to_owned())),
        )],
        body: None,
        timeout: Duration::from_secs(5),
    };
    let response = f.http.request(&request).expect("scripted");
    assert_eq!(response.status, 200);
    assert_eq!(f.http.pending(), 0);
    assert!(
        matches!(f.http.request(&request), Err(HttpError::Other(_))),
        "script used up"
    );
    let logged = format!("{:?}", f.http.requests());
    assert!(
        !logged.contains("test-token-not-real"),
        "secret leaked into Debug: {logged}"
    );
}

#[test]
fn files_list_glob_tail_and_size_limits() {
    let f = Fakes::new(t0());
    let dir = Path::new(r"C:\Users\tester\.codex\sessions\2026\10\10");
    f.fs.add_file(
        dir.join("rollout-a.jsonl"),
        b"line1\nline2\n".to_vec(),
        t0(),
    );
    f.fs.add_file(dir.join("notes.txt"), b"x".to_vec(), t0());
    f.fs.add_file(dir.join(r"sub\deeper.jsonl"), b"y".to_vec(), t0());

    let names: Vec<_> =
        f.fs.list(dir)
            .expect("list")
            .into_iter()
            .map(|e| e.path)
            .collect();
    assert_eq!(
        names,
        [
            dir.join("notes.txt"),
            dir.join("rollout-a.jsonl"),
            dir.join("sub")
        ]
    );
    let rollouts = f.fs.glob(dir, "rollout-*.jsonl").expect("glob");
    assert_eq!(rollouts.len(), 1);
    assert_eq!(
        f.fs.read_tail(&dir.join("rollout-a.jsonl"), 6)
            .expect("tail"),
        b"line2\n"
    );
    let too_big =
        f.fs.read(&dir.join("rollout-a.jsonl"), 4)
            .expect_err("limit");
    assert_eq!(too_big.kind(), ErrorKind::InvalidData);
    let mut streamed = String::new();
    f.fs.open(&dir.join("rollout-a.jsonl"), 64)
        .expect("open")
        .read_to_string(&mut streamed)
        .expect("stream");
    assert_eq!(streamed, "line1\nline2\n");
    assert_eq!(
        f.fs.open(&dir.join("rollout-a.jsonl"), 4)
            .err()
            .map(|e| e.kind()),
        Some(ErrorKind::InvalidData)
    );
    assert!(f.fs.metadata(&dir.join("sub")).expect("dir").is_dir);
    assert_eq!(
        f.fs.metadata(&dir.join("missing"))
            .expect_err("missing")
            .kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn secrets_round_trip_by_target_name() {
    let f = Fakes::new(t0());
    let name = CredentialName {
        provider: ProviderId::Meshy,
        account: AccountKey::primary(),
        name: "api_key".to_owned(),
    };
    assert_eq!(name.target(), "Lastdrop:meshy:default:api_key");
    assert_eq!(f.secrets.get(&name).expect("get"), None);
    f.secrets
        .set(
            &name,
            &SecretString::new("msy-test-key-not-real".to_owned()),
        )
        .expect("set");
    assert_eq!(
        f.secrets.get(&name).expect("get").expect("some").expose(),
        "msy-test-key-not-real"
    );
    f.secrets.delete(&name).expect("delete");
    assert_eq!(f.secrets.get(&name).expect("get"), None);
}

#[test]
fn spawner_scripts_children_and_records_their_input() {
    let f = Fakes::new(t0());
    f.spawner.script(vec![
        FakeLine::Line(r#"{"id":1,"result":{}}"#.to_owned()),
        FakeLine::Timeout,
        FakeLine::Eof,
    ]);
    let spec = SpawnSpec {
        program: r"C:\Users\tester\bin\codex.exe".into(),
        args: vec!["app-server".into()],
        env: vec![("CODEX_HOME".into(), Some(r"C:\Users\tester\.codex".into()))],
        cwd: None,
    };
    let mut child = f.spawner.spawn(&spec).expect("spawn");
    child
        .write_line(r#"{"id":1,"method":"initialize"}"#)
        .expect("write");
    let t = Duration::from_secs(1);
    assert_eq!(
        child.read_line(t).expect("line").as_deref(),
        Some(r#"{"id":1,"result":{}}"#)
    );
    assert_eq!(
        child.read_line(t).expect_err("timeout").kind(),
        ErrorKind::TimedOut
    );
    assert_eq!(child.read_line(t).expect("eof"), None);
    drop(child);
    assert_eq!(f.spawner.killed(), 1, "dropping a child kills it");
    assert_eq!(f.spawner.written(), [r#"{"id":1,"method":"initialize"}"#]);
    assert_eq!(f.spawner.spawned(), std::slice::from_ref(&spec));
    assert_eq!(
        f.spawner.spawn(&spec).err().map(|e| e.kind()),
        Some(ErrorKind::NotFound)
    );
}

#[test]
fn env_has_a_home_and_only_the_variables_given() {
    let f = Fakes::new(t0());
    assert_eq!(
        f.env.home_dir().as_deref(),
        Some(Path::new(r"C:\Users\tester"))
    );
    assert_eq!(f.env.var("CODEX_HOME"), None);
    let env = f.env.clone().with_var("CODEX_HOME", r"D:\codex-work");
    assert_eq!(
        env.var("CODEX_HOME").as_deref(),
        Some(std::ffi::OsStr::new(r"D:\codex-work"))
    );
}
