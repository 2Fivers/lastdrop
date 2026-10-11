//! Live smoke tests of the Codex provider against this machine's Codex. Ignored by default; run
//! by hand with
//! `cargo test -p lastdrop-providers --test codex_live -- --ignored --nocapture --test-threads=1`.
//!
//! `live_app_server_smoke` prints only the plan type, window lengths, used percentages and reset
//! times; `live_provider_pace_smoke` only the source chosen, the provider's INFO/WARN lines
//! (sources, reasons, minutes), ages, durations, start counts and used percentages — never the
//! path, account, e-mail address or anything else Codex returns. `LocalSpawner` is a minimal
//! local stand-in for `real::RealSpawner` (test only, not shipped).
#![cfg(windows)]
#![expect(
    clippy::print_stdout,
    reason = "a manual smoke test reports the values it read"
)]

use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, AccountRef, SourceKind};
use lastdrop_core::settings::CodexOptions;
use lastdrop_providers::codex::{CodexProvider, app_server, to_snapshot};
use lastdrop_providers::fakes::FakeSecrets;
use lastdrop_providers::io::{ChildIo, Env, SpawnSpec, Spawner};
use lastdrop_providers::real::{
    CREATE_NO_WINDOW, RealClock, RealEnv, RealFs, RealSpawner, UreqHttp,
};
use lastdrop_providers::{AccountSource, FetchContext, Provider};

/// Piped stdin/stdout, stderr discarded, no window; a reader thread turns stdout into lines.
#[derive(Default)]
struct LocalSpawner;

struct LocalChild {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<io::Result<String>>,
    done: bool,
}

impl ChildIo for LocalChild {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(ErrorKind::BrokenPipe, "stdin closed"))?;
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        match self.lines.recv_timeout(timeout) {
            Ok(line) => line.map(Some),
            Err(RecvTimeoutError::Timeout) => Err(io::Error::new(ErrorKind::TimedOut, "timeout")),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        if !self.done {
            self.done = true;
            self.stdin = None;
            // Kills exactly this child (by its handle), then reaps it.
            let killed = self.child.kill();
            self.child.wait()?;
            if let Err(e) = killed
                && e.kind() != ErrorKind::InvalidInput
            {
                return Err(e);
            }
        }
        Ok(())
    }
}

impl Drop for LocalChild {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

impl Spawner for LocalSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        for (key, value) in &spec.env {
            match value {
                Some(v) => command.env(key, v),
                None => command.env_remove(key),
            };
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Box::new(LocalChild {
            child,
            stdin,
            lines: rx,
            done: false,
        }))
    }
}

/// `RealSpawner` (kill-on-close job, no window), counting the starts.
#[derive(Default)]
struct CountingSpawner {
    starts: AtomicUsize,
}

impl Spawner for CountingSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        RealSpawner.spawn(spec)
    }
}

/// Prints the Codex provider's INFO and WARN lines: which source was used and why (secret-free
/// by design: sources, reasons, minutes).
struct CodexLog;

impl log::Log for CodexLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
            && metadata.target().starts_with("lastdrop_providers::codex")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            println!("  log {}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static CODEX_LOG: CodexLog = CodexLog;

/// `%CODEX_HOME%`, else `<home>\.codex`.
fn live_codex_home(env: &RealEnv) -> PathBuf {
    env.var("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env.home_dir().map(|h| h.join(".codex")))
        .expect("a Codex folder")
}

#[test]
#[ignore = "live: reads this machine's Codex rollout and may start its codex app-server once"]
fn live_provider_pace_smoke() {
    if log::set_logger(&CODEX_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let env = RealEnv;
    let account = AccountSource::Codex {
        account: AccountKey::primary(),
        codex_home: live_codex_home(&env),
    };
    let (clock, http, fs, secrets, spawner) = (
        RealClock,
        UreqHttp::new(),
        RealFs,
        FakeSecrets::new(),
        CountingSpawner::default(),
    );
    let provider = CodexProvider::new(CodexOptions::default());

    // Twice in a row, as two scheduled fetches a minute apart would: the second never starts
    // another app-server (a fresh rollout, the first start's answer, or a newer rollout).
    for round in 1..=2 {
        let ctx = FetchContext {
            clock: &clock,
            http: &http,
            fs: &fs,
            secrets: &secrets,
            spawner: &spawner,
            env: &env,
            deadline: Timestamp::now() + SignedDuration::from_secs(20),
            manual: false,
        };
        let began = Instant::now();
        println!("fetch {round}:");
        match provider.fetch(&ctx, &account) {
            Ok(snap) => {
                println!(
                    "  source {:?}, health {:?}, reading {} min old, {} ms, starts so far {}",
                    snap.source,
                    snap.health,
                    Timestamp::now().duration_since(snap.measured_at).as_mins(),
                    began.elapsed().as_millis(),
                    spawner.starts.load(Ordering::SeqCst)
                );
                for m in &snap.meters {
                    println!("  meter {}: used {:.0}%", m.id.0, m.used * 100.0);
                }
            }
            Err(e) => println!(
                "  error: {e}; {} ms, starts so far {}",
                began.elapsed().as_millis(),
                spawner.starts.load(Ordering::SeqCst)
            ),
        }
    }
    assert!(
        spawner.starts.load(Ordering::SeqCst) <= 1,
        "the second fetch started another app-server"
    );
}

#[test]
#[ignore = "live: starts this machine's codex app-server (manual smoke test)"]
fn live_app_server_smoke() {
    let env = RealEnv;
    let codex_home = live_codex_home(&env);
    let (clock, http, fs, secrets, spawner) = (
        RealClock,
        UreqHttp::new(),
        RealFs,
        FakeSecrets::new(),
        LocalSpawner,
    );
    let ctx = FetchContext {
        clock: &clock,
        http: &http,
        fs: &fs,
        secrets: &secrets,
        spawner: &spawner,
        env: &env,
        deadline: Timestamp::now() + SignedDuration::from_secs(20),
        manual: false,
    };
    let exe = app_server::find_codex_exe(&ctx, None).expect("codex.exe on this machine");

    let started = Instant::now();
    let usage = app_server::fetch_with(&ctx, &exe, &codex_home);
    let took = started.elapsed();

    match &usage {
        Ok(u) => {
            println!("plan: {}", u.plan.as_deref().unwrap_or("(none)"));
            for (name, w) in [("primary", &u.primary), ("secondary", &u.secondary)] {
                match w {
                    Some(w) => println!(
                        "{name}: window {} min, used {}%, resets {}",
                        w.window_minutes
                            .map_or_else(|| "?".to_owned(), |m| m.to_string()),
                        w.used_percent,
                        w.resets_at
                            .map_or_else(|| "(none)".to_owned(), |t| t.to_string())
                    ),
                    None => println!("{name}: (none)"),
                }
            }
            let snap = to_snapshot(
                u,
                SourceKind::CodexAppServer,
                AccountRef::primary(),
                Timestamp::now(),
            );
            for m in &snap.meters {
                println!(
                    "meter {}: used {:.0}%, resets {}",
                    m.id.0,
                    m.used * 100.0,
                    m.resets_at
                        .map_or_else(|| "(none)".to_owned(), |t| t.to_string())
                );
            }
        }
        Err(e) => println!("error: {e}"),
    }
    assert!(took < Duration::from_secs(15), "init 8 s + 2 × 3 s at most");
    usage.expect("the app-server answered");
}
