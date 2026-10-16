//! Google Antigravity: the 5-hour and weekly quotas of each model pool (`Gemini Models`,
//! `Claude and GPT models`).
//!
//! Source (ARCHITECTURE provider table): the Antigravity CLI's own print mode,
//! `agy -p /usage --output-format json` (agy 1.1.11 and later, official changelog), run without a
//! window in its own kill-on-close job (agy may start MCP servers). agy uses and renews its own
//! sign-in; Lastdrop handles no token. The JSON shape is not documented: it follows the
//! reference tools' parsers (Win-CodexBar, CodexBar) and accepts their spellings:
//!
//! `{status: "SUCCESS", command: {name: "usage", data: {groups: [{name|displayName, buckets:
//! [{id|bucketId, name|displayName, window: "5h"|"weekly", remaining_fraction |
//! remainingFraction | remaining.remainingFraction | remaining {case: "remainingFraction",
//! value}, reset_time|resetTime, disabled}]}]}}}`
//!
//! used = 1 − remaining fraction. A bucket that is disabled or has no fraction is unknown and
//! becomes no meter (never 0 %).

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, Meter, MeterId, MeterNames, PoolWindow, ProviderId, Snapshot,
    SourceKind, WindowLen,
};
use serde_json::{Map, Value};

use crate::account::AccountSource;
use crate::claude::json::instant_of;
use crate::error::FetchError;
use crate::io::{SpawnSpec, TerminalCommand};
use crate::{FetchContext, Provider};

/// The variable that names agy's executable; when set, nothing else is looked for.
pub const CLI_PATH_VAR: &str = "ANTIGRAVITY_CLI_PATH";

/// agy's install folder under `%LOCALAPPDATA%` (Google's `install.ps1`).
pub const INSTALL_DIR: &str = r"agy\bin";

/// A variable that would hand agy a sign-in from outside; removed from its environment.
pub const CREDENTIALS_VAR: &str = "ANTIGRAVITY_OAUTH_CREDENTIALS_JSON";

/// The first agy with print mode (`-p /usage` answers without starting a turn).
pub const MIN_VERSION: (u32, u32, u32) = (1, 1, 11);

/// How long `agy --version` may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// The most output read (agy's answer is a few kilobytes).
pub const MAX_OUTPUT: usize = 1 << 20;

/// Lower bound between runs: 10 minutes (**not from the spec**: each run starts agy, which takes
/// seconds and may start MCP servers).
pub const MIN_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Antigravity's usage page in the desktop app's docs, opened from a card.
pub const USAGE_PAGE: &str = "https://antigravity.google/docs/plans";

/// The source every Antigravity reading and error carries.
const SOURCE: SourceKind = SourceKind::AntigravityCli;

/// One bucket: a window of a model pool.
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    /// The pool as Antigravity names it (`Gemini Models`).
    pub pool: String,
    /// Its place among the pools of the answer.
    pub pool_index: u16,
    /// The window.
    pub window: PoolWindow,
    /// The share used, 0–1.
    pub used: f64,
    /// When the window resets.
    pub resets_at: Option<Timestamp>,
}

/// The Antigravity provider. Remembers the agy it found to be new enough, so `agy --version`
/// runs once per executable.
#[derive(Debug, Default)]
pub struct AntigravityProvider {
    checked: Mutex<Option<PathBuf>>,
}

impl AntigravityProvider {
    /// The provider.
    pub fn new() -> AntigravityProvider {
        AntigravityProvider::default()
    }

    /// Whether `exe` is new enough, asking it once.
    fn new_enough(&self, ctx: &FetchContext<'_>, exe: &Path) -> Result<(), FetchError> {
        let known = self
            .checked
            .lock()
            .map(|c| c.as_deref() == Some(exe))
            .unwrap_or(false);
        if known {
            return Ok(());
        }
        let version = run(ctx, exe, &["--version"], VERSION_TIMEOUT)?
            .lines()
            .find_map(parse_version);
        match version {
            Some(v) if v >= MIN_VERSION => {
                if let Ok(mut c) = self.checked.lock() {
                    *c = Some(exe.to_path_buf());
                }
                Ok(())
            }
            Some((a, b, c)) => Err(FetchError::Unavailable {
                detail: format!("agy {a}.{b}.{c} is too old; update it to 1.1.11 or later"),
            }),
            None => Err(FetchError::Unavailable {
                detail: "agy did not say its version".to_owned(),
            }),
        }
    }
}

impl Provider for AntigravityProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Antigravity
    }

    /// agy found and new enough → `agy -p /usage --output-format json` → parse.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Antigravity { account } = account else {
            return Err(FetchError::Unavailable {
                detail: "not an Antigravity account".to_owned(),
            });
        };
        let result = find_agy(ctx)
            .ok_or_else(|| FetchError::Unavailable {
                detail: "the Antigravity CLI (agy) not found".to_owned(),
            })
            .and_then(|exe| {
                self.new_enough(ctx, &exe)?;
                let output = run(ctx, &exe, &usage_args(), ctx.remaining())?;
                parse_report(&output)
            });
        match result {
            Ok(buckets) => Ok(to_snapshot(&buckets, account.clone(), ctx.clock.now())),
            Err(e) => {
                log::warn!("antigravity: {e}");
                Err(e)
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// `agy` in a terminal: it asks for the browser sign-in when it has none.
    fn sign_in_command(&self, _account: &AccountSource) -> Option<TerminalCommand> {
        Some(TerminalCommand {
            program: "agy".into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
        })
    }

    fn usage_page(&self) -> Option<&'static str> {
        Some(USAGE_PAGE)
    }
}

// ---- agy ----------------------------------------------------------------------------------------

fn is_file(ctx: &FetchContext<'_>, path: &Path) -> bool {
    ctx.fs.metadata(path).is_ok_and(|m| !m.is_dir)
}

/// agy's executable: `ANTIGRAVITY_CLI_PATH` when set (nothing else then), else
/// `%LOCALAPPDATA%\agy\bin\agy.exe`, else the first `agy.exe` on `PATH`.
pub fn find_agy(ctx: &FetchContext<'_>) -> Option<PathBuf> {
    if let Some(path) = ctx.env.var(CLI_PATH_VAR).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        return (path.is_absolute() && is_file(ctx, &path)).then_some(path);
    }
    let installed = ctx
        .env
        .var("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|l| PathBuf::from(l).join(INSTALL_DIR).join("agy.exe"))
        .filter(|p| p.is_absolute() && is_file(ctx, p));
    installed.or_else(|| {
        let path = ctx.env.var("PATH")?;
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("agy.exe"))
            .find(|candidate| is_file(ctx, candidate))
    })
}

/// The print-mode arguments. No `--print-timeout`: it may be newer than 1.1.11, and the fetch's
/// own deadline ends a run that takes too long.
pub fn usage_args() -> Vec<&'static str> {
    vec!["-p", "/usage", "--output-format", "json"]
}

/// How agy runs: without the outside sign-in variable, in its own folder (so no project's
/// configuration is picked up).
pub fn spec(exe: &Path, args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        program: exe.to_path_buf(),
        args: args.iter().map(OsString::from).collect(),
        env: vec![(CREDENTIALS_VAR.into(), None)],
        cwd: exe.parent().map(Path::to_path_buf),
    }
}

/// Runs agy with `args` and returns its standard output (at most [`MAX_OUTPUT`] bytes), within
/// `limit` and the fetch's deadline.
fn run(
    ctx: &FetchContext<'_>,
    exe: &Path,
    args: &[&str],
    limit: Duration,
) -> Result<String, FetchError> {
    let timed_out = || FetchError::Transient {
        from: SOURCE,
        detail: "agy did not answer in time".to_owned(),
        retry_after: None,
    };
    let mut child = ctx.spawner.spawn(&spec(exe, args)).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            FetchError::Unavailable {
                detail: "the Antigravity CLI (agy) not found".to_owned(),
            }
        } else {
            FetchError::Transient {
                from: SOURCE,
                detail: "agy could not start".to_owned(),
                retry_after: None,
            }
        }
    })?;
    let deadline = ctx.clock.now()
        + SignedDuration::try_from(limit.min(ctx.remaining())).unwrap_or(SignedDuration::ZERO);
    let mut output = String::new();
    let result = loop {
        let left =
            Duration::try_from(deadline.duration_since(ctx.clock.now())).unwrap_or(Duration::ZERO);
        if left.is_zero() {
            break Err(timed_out());
        }
        match child.read_line(left) {
            Ok(Some(line)) => {
                if output.len() + line.len() + 1 > MAX_OUTPUT {
                    break Err(FetchError::Parse {
                        from: SOURCE,
                        detail: "agy printed too much".to_owned(),
                    });
                }
                output.push_str(&line);
                output.push('\n');
                // Done once the answer is complete: an MCP server agy started may hold the pipe
                // open after agy is done.
                if complete(&output) {
                    break Ok(output);
                }
            }
            Ok(None) => break Ok(output),
            Err(e) if e.kind() == ErrorKind::TimedOut => break Err(timed_out()),
            Err(_) => {
                break Err(FetchError::Transient {
                    from: SOURCE,
                    detail: "agy failed".to_owned(),
                    retry_after: None,
                });
            }
        }
    };
    let _ = child.kill();
    result
}

/// Whether `output` holds a whole answer: a version line, or a JSON object that parses.
fn complete(output: &str) -> bool {
    let text = output.trim();
    if text.lines().count() == 1 && parse_version(text).is_some() {
        return true;
    }
    match text.find('{') {
        Some(start) if text.ends_with('}') => {
            serde_json::from_str::<serde::de::IgnoredAny>(&text[start..]).is_ok()
        }
        _ => false,
    }
}

/// agy's own version in a line: the first token that is `x.y.z` (strictly three numbers), alone
/// or after `agy/`, `agy-` or `v` (`agy 1.2.15`, `v1.2.15`, `agy/1.2.15 win32-x64`). Tokens after
/// one naming another program (`node-v20.11.1`) are never taken.
pub fn parse_version(line: &str) -> Option<(u32, u32, u32)> {
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    for token in line.split(|c: char| c.is_whitespace() || c == ',') {
        let lower = token.to_ascii_lowercase();
        if lower.starts_with("node") || lower.starts_with("go") || lower.starts_with("electron") {
            return None;
        }
        let bare = ["agy/", "agy-", "v"]
            .iter()
            .find_map(|p| lower.strip_prefix(p))
            .unwrap_or(&lower);
        let mut parts = bare.split('.');
        let (Some(a), Some(b), Some(c), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if let (Some(a), Some(b), Some(c)) = (number(a), number(b), number(c)) {
            return Some((a, b, c));
        }
    }
    None
}

// ---- The report ---------------------------------------------------------------------------------

fn parse_error(detail: &str) -> FetchError {
    FetchError::Parse {
        from: SOURCE,
        detail: detail.to_owned(),
    }
}

/// The first of `keys` that `object` has as a string.
fn text<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| object.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// The remaining fraction of a bucket in any of its spellings.
fn remaining_fraction(bucket: &Map<String, Value>) -> Option<f64> {
    let number = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|n| n.is_finite());
    number(bucket.get("remaining_fraction"))
        .or_else(|| number(bucket.get("remainingFraction")))
        .or_else(|| {
            let remaining = bucket.get("remaining")?.as_object()?;
            number(remaining.get("remainingFraction")).or_else(|| {
                (remaining.get("case").and_then(Value::as_str) == Some("remainingFraction"))
                    .then(|| number(remaining.get("value")))
                    .flatten()
            })
        })
}

/// The window of a bucket: `window` (`5h`, `weekly`), else its name or id.
fn window_of(bucket: &Map<String, Value>) -> Option<PoolWindow> {
    let named = |t: &str| {
        let t = t.to_ascii_lowercase();
        if t.contains("week") {
            Some(PoolWindow::Weekly)
        } else if t.contains("5h") || t.contains("5-hour") || t.contains("five") || t == "5 h" {
            Some(PoolWindow::FiveHour)
        } else {
            None
        }
    };
    text(bucket, &["window"])
        .and_then(named)
        .or_else(|| text(bucket, &["id", "bucketId", "name", "displayName"]).and_then(named))
}

/// The pools of the report: `command.data.groups`, else the quota summary's `groups`
/// (`response`/`summary`, or top level).
fn groups(top: &Map<String, Value>) -> Option<&Vec<Value>> {
    fn at(o: Option<&Value>) -> Option<&Vec<Value>> {
        o?.as_object()?.get("groups")?.as_array()
    }
    at(top.get("command").and_then(|c| c.get("data")))
        .or_else(|| at(top.get("response")))
        .or_else(|| at(top.get("summary")))
        .or_else(|| top.get("groups")?.as_array())
}

/// The buckets of agy's JSON report. Errors: not JSON, a status other than `SUCCESS` (a sign-in
/// problem is [`FetchError::Auth`]), or no bucket with a fraction.
pub fn parse_report(output: &str) -> Result<Vec<Bucket>, FetchError> {
    // agy may print a line before the JSON: read from the first `{`.
    let start = output
        .find('{')
        .ok_or_else(|| parse_error("agy printed no JSON"))?;
    let value: Value = serde_json::from_str(output[start..].trim_end())
        .map_err(|_| parse_error("agy printed something other than its JSON report"))?;
    let top = value
        .as_object()
        .ok_or_else(|| parse_error("agy's report is not a JSON object"))?;
    if let Some(status) = text(top, &["status"])
        && !status.eq_ignore_ascii_case("SUCCESS")
    {
        let message = text(top, &["message", "error"])
            .unwrap_or("")
            .to_ascii_lowercase();
        let signed_out = [
            "auth",
            "sign in",
            "sign-in",
            "login",
            "log in",
            "credential",
        ]
        .iter()
        .any(|w| message.contains(w) || status.to_ascii_lowercase().contains(w));
        return Err(if signed_out {
            FetchError::Auth { from: SOURCE }
        } else {
            FetchError::Transient {
                from: SOURCE,
                detail: "agy answered with an error".to_owned(),
                retry_after: None,
            }
        });
    }
    let mut buckets = Vec::new();
    for (index, group) in groups(top).into_iter().flatten().enumerate() {
        let Some(group) = group.as_object() else {
            continue;
        };
        let pool = text(group, &["displayName", "name"]).unwrap_or("Models");
        let pool_index = u16::try_from(index).unwrap_or(u16::MAX);
        for bucket in group
            .get("buckets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let (Some(window), Some(fraction)) = (window_of(bucket), remaining_fraction(bucket))
            else {
                continue;
            };
            let resets_at = ["reset_time", "resetTime"]
                .iter()
                .find_map(|k| bucket.get(*k).and_then(instant_of));
            buckets.push(Bucket {
                pool: pool.to_owned(),
                pool_index,
                window,
                used: (1.0 - fraction.clamp(0.0, 1.0)).max(0.0),
                resets_at,
            });
        }
    }
    if buckets.is_empty() {
        return Err(parse_error("agy's report has no quota"));
    }
    Ok(buckets)
}

// ---- The snapshot -------------------------------------------------------------------------------

/// The id part of a pool name: lower case letters and digits, words joined by `_`
/// (`Claude and GPT models` → `claude_and_gpt_models`).
pub fn pool_slug(pool: &str) -> String {
    let words: Vec<String> = pool
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "models".to_owned()
    } else {
        words.join("_")
    }
}

/// The meter of a bucket: `<pool>:five_hour` or `<pool>:weekly`.
pub fn meter(bucket: &Bucket) -> Meter {
    let (suffix, length, step) = match bucket.window {
        PoolWindow::FiveHour => (MeterId::FIVE_HOUR, SignedDuration::from_hours(5), 0),
        PoolWindow::Weekly => (MeterId::WEEKLY, SignedDuration::from_hours(7 * 24), 1),
    };
    Meter {
        id: MeterId::new(&format!("{}:{suffix}", pool_slug(&bucket.pool))),
        names: MeterNames::pool_window(&bucket.pool, bucket.window),
        order: bucket.pool_index.saturating_mul(2).saturating_add(step),
        used: bucket.used,
        limited: bucket.used >= 1.0,
        resets_at: bucket.resets_at,
        window: WindowLen::Fixed(length),
        starts_on_use: false,
    }
}

/// The snapshot of a report.
pub fn to_snapshot(buckets: &[Bucket], account: AccountKey, fetched_at: Timestamp) -> Snapshot {
    Snapshot {
        provider: ProviderId::Antigravity,
        account: AccountRef {
            key: account,
            ..AccountRef::primary()
        },
        source: SOURCE,
        // A live report: the values are true when read.
        measured_at: fetched_at,
        fetched_at,
        meters: buckets.iter().map(meter).collect(),
        balances: Vec::new(),
        health: Health::Ok,
    }
}
