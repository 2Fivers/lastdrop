//! Live smoke test of the Codex app-server source against this machine's Codex. Ignored by
//! default; run by hand with
//! `cargo test -p lastdrop-providers --test codex_live -- --ignored --nocapture`.
//!
//! It prints only the plan type, window lengths, used percentages and reset times — never the
//! path, account, e-mail address or anything else Codex returns. The spawner here is a minimal
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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountRef, SourceKind};
use lastdrop_providers::FetchContext;
use lastdrop_providers::codex::{app_server, to_snapshot};
use lastdrop_providers::fakes::FakeSecrets;
use lastdrop_providers::io::{ChildIo, Env, SpawnSpec, Spawner};
use lastdrop_providers::real::{CREATE_NO_WINDOW, RealClock, RealEnv, RealFs, UreqHttp};

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

#[test]
#[ignore = "live: starts this machine's codex app-server (manual smoke test)"]
fn live_app_server_smoke() {
    let env = RealEnv;
    let codex_home = env
        .var("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env.home_dir().map(|h| h.join(".codex")))
        .expect("a Codex folder");
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
