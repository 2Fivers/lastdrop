//! Source 1: `codex app-server` (JSON-RPC 2.0, one JSON object per line over stdio).
//!
//! Sequence: spawn (no window, `CODEX_HOME` set) → `initialize` request (8 s) → `initialized`
//! notification → `account/read` (3 s) → `account/rateLimits/read` (3 s) → end (stdin closed,
//! up to `real::EXIT_GRACE` to exit by itself, then terminated). Lines that are not the awaited
//! response (notifications, server requests, logs) are skipped. The app-server renews Codex's
//! own sign-in, which is why it is never terminated before it had the chance to exit on its own;
//! Lastdrop never touches `auth.json`.
//!
//! Wire shapes (checked against `codex app-server generate-json-schema` of codex-cli 0.162):
//! messages omit the `"jsonrpc"` field; `account/read` answers
//! `{account: {type: "chatgpt", email, planType} | {type: "apiKey"} | {type: "amazonBedrock"} |
//! null, requiresOpenaiAuth}`; `account/rateLimits/read` answers `{rateLimits: Snapshot,
//! rateLimitsByLimitId: {codex: Snapshot, …} | null, …}` with `Snapshot = {limitId, primary:
//! {usedPercent, windowDurationMins, resetsAt (Unix s)} | null, secondary, planType, …}`. Only
//! the `type` and `planType` of the account are read; the e-mail address is never kept.

use std::ffi::OsString;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::SourceKind;
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use super::{CodexUsage, plan_name, window_from};
use crate::FetchContext;
use crate::error::FetchError;
use crate::io::{ChildIo, SpawnSpec};

/// Time allowed for `initialize` (ARCHITECTURE).
pub const INIT_TIMEOUT: Duration = Duration::from_secs(8);

/// Time allowed per call after initialization (ARCHITECTURE).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(3);

/// The rate-limits method.
pub const METHOD_RATE_LIMITS: &str = "account/rateLimits/read";

/// The account method (sign-in state and plan).
pub const METHOD_ACCOUNT: &str = "account/read";

/// The handshake request.
pub const METHOD_INITIALIZE: &str = "initialize";

/// The notification that completes the handshake.
pub const METHOD_INITIALIZED: &str = "initialized";

/// Client name sent in `initialize` (`clientInfo.name`).
pub const CLIENT_NAME: &str = "lastdrop";

/// Client version sent in `initialize` (`clientInfo.version`).
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The environment variable that selects Codex's home folder (account).
pub const CODEX_HOME_VAR: &str = "CODEX_HOME";

/// JSON-RPC id of `initialize`.
pub const ID_INITIALIZE: u64 = 1;

/// JSON-RPC id of `account/read`.
pub const ID_ACCOUNT: u64 = 2;

/// JSON-RPC id of `account/rateLimits/read`.
pub const ID_RATE_LIMITS: u64 = 3;

/// Lines skipped while waiting for one response before giving up (a chatty server cannot keep a
/// fetch busy until its timeout).
const MAX_SKIPPED_LINES: usize = 1000;

/// Where the Codex desktop app keeps its bundled CLI, relative to `%LOCALAPPDATA%`:
/// `OpenAI\Codex\bin\<hash>\codex.exe` (the hash changes with every update).
pub const BUNDLED_BIN_DIR: &str = r"OpenAI\Codex\bin";

// ---- Finding codex.exe --------------------------------------------------------------------------

fn is_file(ctx: &FetchContext<'_>, path: &Path) -> bool {
    ctx.fs.metadata(path).is_ok_and(|m| !m.is_dir)
}

fn non_empty_var(ctx: &FetchContext<'_>, name: &str) -> Option<OsString> {
    ctx.env.var(name).filter(|v| !v.is_empty())
}

/// `%LOCALAPPDATA%`, else `<home>\AppData\Local`.
fn local_app_data(ctx: &FetchContext<'_>) -> Option<PathBuf> {
    non_empty_var(ctx, "LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| ctx.env.home_dir().map(|h| h.join("AppData").join("Local")))
}

/// The newest (by modification time) `%LOCALAPPDATA%\OpenAI\Codex\bin\*\codex.exe`; folders
/// without a `codex.exe` are skipped.
fn newest_bundled(ctx: &FetchContext<'_>) -> Option<PathBuf> {
    let bin = local_app_data(ctx)?.join(BUNDLED_BIN_DIR);
    let entries = ctx.fs.list(&bin).ok()?;
    entries
        .into_iter()
        .filter(|e| e.meta.is_dir)
        .filter_map(|dir| {
            let exe = dir.path.join("codex.exe");
            let meta = ctx.fs.metadata(&exe).ok()?;
            (!meta.is_dir).then_some((meta.modified, exe))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, exe)| exe)
}

/// The first `name` in an absolute `PATH` folder.
fn on_path(ctx: &FetchContext<'_>, name: &str) -> Option<PathBuf> {
    let path = non_empty_var(ctx, "PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_file(ctx, candidate))
}

/// Finds `codex.exe`: the settings override when given and present, else the newest bundled
/// one, else the first on `PATH`. `None` when there is none.
///
/// Bundled = the Codex desktop app's `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`
/// ([`BUNDLED_BIN_DIR`]), newest by modification time (the copy inside the VS Code extension
/// is not searched yet). After `codex.exe` on `PATH` comes `codex.cmd` on `PATH` (the npm
/// install), which [`launch_spec`] starts through `cmd.exe`.
pub fn find_codex_exe(ctx: &FetchContext<'_>, override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path {
        if is_file(ctx, path) {
            return Some(path.to_path_buf());
        }
        log::warn!(
            "codex: the configured codex.exe ({}) does not exist; looking elsewhere",
            path.display()
        );
    }
    newest_bundled(ctx)
        .or_else(|| on_path(ctx, "codex.exe"))
        .or_else(|| on_path(ctx, "codex.cmd"))
}

// ---- Spawning -----------------------------------------------------------------------------------

/// `codex app-server` with `CODEX_HOME` = `codex_home`.
///
/// The working directory is `codex_home`, so the child never inherits (or holds open) whatever
/// folder Lastdrop was started in.
pub fn spawn_spec(exe: &Path, codex_home: &Path) -> SpawnSpec {
    SpawnSpec {
        program: exe.to_path_buf(),
        args: vec!["app-server".into()],
        env: vec![(
            CODEX_HOME_VAR.into(),
            Some(codex_home.as_os_str().to_owned()),
        )],
        cwd: Some(codex_home.to_path_buf()),
    }
}

/// `cmd.exe /d /c <script> app-server` for an npm `codex.cmd`: like [`spawn_spec`] but through
/// the command interpreter `comspec` (`/d`: no AutoRun commands that could print to stdout).
pub fn spawn_spec_via_cmd(comspec: &Path, script: &Path, codex_home: &Path) -> SpawnSpec {
    SpawnSpec {
        program: comspec.to_path_buf(),
        args: vec![
            "/d".into(),
            "/c".into(),
            script.as_os_str().to_owned(),
            "app-server".into(),
        ],
        ..spawn_spec(script, codex_home)
    }
}

/// The command interpreter: `%ComSpec%` when absolute, else `%SystemRoot%\System32\cmd.exe`,
/// else `C:\Windows\System32\cmd.exe`.
pub fn command_interpreter(ctx: &FetchContext<'_>) -> PathBuf {
    non_empty_var(ctx, "ComSpec")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            non_empty_var(ctx, "SystemRoot")
                .map(|root| PathBuf::from(root).join("System32").join("cmd.exe"))
        })
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe"))
}

/// [`spawn_spec`] for an executable, [`spawn_spec_via_cmd`] for a `.cmd`/`.bat` script.
pub fn launch_spec(ctx: &FetchContext<'_>, exe: &Path, codex_home: &Path) -> SpawnSpec {
    let is_script = exe
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if is_script {
        spawn_spec_via_cmd(&command_interpreter(ctx), exe, codex_home)
    } else {
        spawn_spec(exe, codex_home)
    }
}

// ---- Messages -----------------------------------------------------------------------------------

/// The `initialize` request line with JSON-RPC id `id`.
pub fn initialize_request(id: u64) -> String {
    json!({
        "id": id,
        "method": METHOD_INITIALIZE,
        "params": {
            "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION }
        }
    })
    .to_string()
}

/// The `initialized` notification line.
pub fn initialized_notification() -> String {
    json!({ "method": METHOD_INITIALIZED }).to_string()
}

/// The `account/read` request line with id `id` (`refreshToken: false`: never ask Codex to
/// refresh on Lastdrop's behalf).
pub fn account_request(id: u64) -> String {
    json!({ "id": id, "method": METHOD_ACCOUNT, "params": { "refreshToken": false } }).to_string()
}

/// The `account/rateLimits/read` request line with id `id`.
///
/// `excludeResetCreditDetails: true` (a background poll; skips the reset-credit lookup). Never
/// `supportsLunaReserve`, which would let the backend record an experiment exposure.
pub fn rate_limits_request(id: u64) -> String {
    json!({
        "id": id,
        "method": METHOD_RATE_LIMITS,
        "params": { "excludeResetCreditDetails": true }
    })
    .to_string()
}

/// The fields that say whether a line is a response, and to what.
#[derive(Deserialize)]
struct Header {
    id: Option<Value>,
    method: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct Envelope {
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: Option<i64>,
    message: Option<String>,
}

/// A JSON-RPC error response, reduced to what classifies it (the message is never shown or
/// logged: it may quote a server body).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ErrorReply {
    code: Option<i64>,
    message: String,
}

fn id_matches(id: &Value, want: u64) -> bool {
    match id {
        Value::Number(n) => n.as_u64() == Some(want),
        Value::String(s) => s.trim() == want.to_string(),
        _ => false,
    }
}

/// `Ok(None)`: not the response to `id` (not JSON, a notification, a server request, another
/// id). `Ok(Some(Ok(result)))`: the result (`Null` when absent). `Ok(Some(Err(_)))`: an error
/// response. `Err`: the response to `id` but not readable.
fn response(line: &str, id: u64) -> Result<Option<Result<Value, ErrorReply>>, FetchError> {
    let line = line.trim();
    if !line.starts_with('{') {
        return Ok(None);
    }
    let Ok(header) = serde_json::from_str::<Header>(line) else {
        return Ok(None);
    };
    // Server requests carry an id *and* a method; only responses lack the method.
    if header.method.is_some() || !header.id.as_ref().is_some_and(|v| id_matches(v, id)) {
        return Ok(None);
    }
    let envelope = serde_json::from_str::<Envelope>(line).map_err(|_| FetchError::Parse {
        from: SourceKind::CodexAppServer,
        detail: "unreadable app-server response".to_owned(),
    })?;
    Ok(Some(match envelope.error {
        Some(e) => Err(ErrorReply {
            code: e.code,
            message: e.message.unwrap_or_default(),
        }),
        None => Ok(envelope.result.unwrap_or(Value::Null)),
    }))
}

/// Classifies an error response: sign-in problems → Auth, throttling → RateLimited, anything
/// else → Transient with the error code only.
fn classify(error: &ErrorReply) -> FetchError {
    let from = SourceKind::CodexAppServer;
    let m = error.message.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    if has(&[
        "authentication required",
        "not logged in",
        "not signed in",
        "unauthorized",
        "401",
        "log in again",
        "login again",
        "sign in again",
        "refresh_token",
        "refresh token",
    ]) {
        FetchError::Auth { from }
    } else if has(&["429", "too many requests"]) {
        FetchError::RateLimited {
            from,
            retry_after: None,
        }
    } else {
        FetchError::Transient {
            from,
            detail: match error.code {
                Some(code) => format!("app-server error {code}"),
                None => "app-server error".to_owned(),
            },
            retry_after: None,
        }
    }
}

fn parse_error(detail: &str) -> FetchError {
    FetchError::Parse {
        from: SourceKind::CodexAppServer,
        detail: detail.to_owned(),
    }
}

/// The usage in an `account/rateLimits/read` result: the `codex` bucket of
/// `rateLimitsByLimitId` when present, else `rateLimits`.
fn usage_from_result(result: &Value) -> Result<CodexUsage, FetchError> {
    let snapshot = result
        .get("rateLimitsByLimitId")
        .and_then(|by_id| by_id.get("codex"))
        .filter(|s| s.is_object())
        .or_else(|| result.get("rateLimits").filter(|s| s.is_object()))
        .ok_or_else(|| parse_error("missing rateLimits"))?;
    let keys = ("usedPercent", "windowDurationMins", "resetsAt");
    let usage = CodexUsage {
        primary: snapshot
            .get("primary")
            .and_then(|w| window_from(w, keys, None)),
        secondary: snapshot
            .get("secondary")
            .and_then(|w| window_from(w, keys, None)),
        plan: snapshot
            .get("planType")
            .and_then(Value::as_str)
            .and_then(plan_name),
        measured_at: None,
    };
    if usage.primary.is_none() && usage.secondary.is_none() {
        return Err(parse_error("no rate-limit windows"));
    }
    Ok(usage)
}

/// Parses one stdout line: `Ok(None)` when it is not the response to `id`; the usage when it is;
/// an error response maps to Auth (not signed in) or Transient.
pub fn parse_rate_limits(line: &str, id: u64) -> Result<Option<CodexUsage>, FetchError> {
    match response(line, id)? {
        None => Ok(None),
        Some(Err(error)) => Err(classify(&error)),
        Some(Ok(result)) => usage_from_result(&result).map(Some),
    }
}

/// What `account/read` says about the account in `CODEX_HOME`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountState {
    /// Signed in with ChatGPT; the plan's display name when known.
    ChatGpt {
        /// [`super::plan_name`] of `planType`.
        plan: Option<String>,
    },
    /// Signed in with an API key: no plan windows to read.
    ApiKey,
    /// Another kind of sign-in (Amazon Bedrock, …): no plan windows to read.
    Other,
    /// Not signed in (`account: null`).
    SignedOut,
}

/// Parses one stdout line for the `account/read` response with `id`, like
/// [`parse_rate_limits`]. Reads only `account.type` and `account.planType`.
pub fn parse_account(line: &str, id: u64) -> Result<Option<AccountState>, FetchError> {
    match response(line, id)? {
        None => Ok(None),
        Some(Err(error)) => Err(classify(&error)),
        Some(Ok(result)) => {
            let account = result.get("account").unwrap_or(&Value::Null);
            if account.is_null() {
                return Ok(Some(AccountState::SignedOut));
            }
            let state = match account.get("type").and_then(Value::as_str) {
                Some("chatgpt") => AccountState::ChatGpt {
                    plan: account
                        .get("planType")
                        .and_then(Value::as_str)
                        .and_then(plan_name),
                },
                Some("apiKey") => AccountState::ApiKey,
                Some(_) => AccountState::Other,
                None => return Err(parse_error("account without a type")),
            };
            Ok(Some(state))
        }
    }
}

/// Whether a line is the (successful) response to `id`; error responses fail.
fn parse_ack(line: &str, id: u64) -> Result<Option<()>, FetchError> {
    match response(line, id)? {
        None => Ok(None),
        Some(Err(error)) => Err(classify(&error)),
        Some(Ok(_)) => Ok(Some(())),
    }
}

// ---- The conversation ---------------------------------------------------------------------------

fn transient(detail: &str) -> FetchError {
    FetchError::Transient {
        from: SourceKind::CodexAppServer,
        detail: detail.to_owned(),
        retry_after: None,
    }
}

fn write_failed(error: &io::Error) -> FetchError {
    log::debug!("codex app-server: write failed: {error}");
    transient("codex app-server exited")
}

/// Time left until `deadline` per the context's clock.
fn left(ctx: &FetchContext<'_>, deadline: Timestamp) -> Duration {
    Duration::try_from(deadline.duration_since(ctx.clock.now())).unwrap_or(Duration::ZERO)
}

/// Reads lines until `parse` recognizes one, within `timeout` (capped by the fetch deadline).
fn await_reply<T>(
    ctx: &FetchContext<'_>,
    child: &mut dyn ChildIo,
    timeout: Duration,
    mut parse: impl FnMut(&str) -> Result<Option<T>, FetchError>,
) -> Result<T, FetchError> {
    let budget = timeout.min(ctx.remaining());
    let deadline = jiff::SignedDuration::try_from(budget)
        .ok()
        .and_then(|d| ctx.clock.now().checked_add(d).ok())
        .unwrap_or(ctx.deadline);
    for _ in 0..=MAX_SKIPPED_LINES {
        let wait = left(ctx, deadline);
        if wait.is_zero() {
            return Err(transient("codex app-server timed out"));
        }
        match child.read_line(wait) {
            Ok(Some(line)) => {
                if let Some(value) = parse(&line)? {
                    return Ok(value);
                }
            }
            Ok(None) => return Err(transient("codex app-server exited")),
            Err(e) if e.kind() == ErrorKind::TimedOut => {
                return Err(transient("codex app-server timed out"));
            }
            Err(e) => {
                log::debug!("codex app-server: read failed: {e}");
                return Err(transient("codex app-server exited"));
            }
        }
    }
    Err(transient("codex app-server did not answer"))
}

/// initialize → initialized → account/read → account/rateLimits/read on a running child.
fn converse(ctx: &FetchContext<'_>, child: &mut dyn ChildIo) -> Result<CodexUsage, FetchError> {
    child
        .write_line(&initialize_request(ID_INITIALIZE))
        .map_err(|e| write_failed(&e))?;
    await_reply(ctx, child, INIT_TIMEOUT, |line| {
        parse_ack(line, ID_INITIALIZE)
    })?;
    child
        .write_line(&initialized_notification())
        .map_err(|e| write_failed(&e))?;

    child
        .write_line(&account_request(ID_ACCOUNT))
        .map_err(|e| write_failed(&e))?;
    let account_plan = match await_reply(ctx, child, CALL_TIMEOUT, |line| {
        parse_account(line, ID_ACCOUNT)
    }) {
        Ok(AccountState::ChatGpt { plan }) => plan,
        Ok(AccountState::SignedOut) => {
            return Err(FetchError::Auth {
                from: SourceKind::CodexAppServer,
            });
        }
        Ok(AccountState::ApiKey) => {
            return Err(FetchError::Unavailable {
                detail: "Codex is signed in with an API key (no plan limits)".to_owned(),
            });
        }
        Ok(AccountState::Other) => {
            return Err(FetchError::Unavailable {
                detail: "Codex is not signed in with ChatGPT (no plan limits)".to_owned(),
            });
        }
        Err(error @ FetchError::Auth { .. }) => return Err(error),
        Err(error) => {
            // The plan is a nicety; the rate limits may still answer.
            log::debug!("codex app-server: account/read failed: {error}");
            None
        }
    };

    child
        .write_line(&rate_limits_request(ID_RATE_LIMITS))
        .map_err(|e| write_failed(&e))?;
    let mut usage = await_reply(ctx, child, CALL_TIMEOUT, |line| {
        parse_rate_limits(line, ID_RATE_LIMITS)
    })?;
    // The fresh rate-limit response wins (a plan change shows there first).
    if usage.plan.is_none() {
        usage.plan = account_plan;
    }
    Ok(usage)
}

/// [`fetch`] with a known `codex.exe` (or `codex.cmd`): check the account folder → spawn →
/// converse → kill (always, also on failure).
pub fn fetch_with(
    ctx: &FetchContext<'_>,
    exe: &Path,
    codex_home: &Path,
) -> Result<CodexUsage, FetchError> {
    // Codex exits at once when CODEX_HOME is not an existing folder.
    if !ctx.fs.metadata(codex_home).is_ok_and(|m| m.is_dir) {
        return Err(FetchError::Unavailable {
            detail: "Codex folder not found".to_owned(),
        });
    }
    if ctx.remaining().is_zero() {
        return Err(transient("timed out"));
    }
    let spec = launch_spec(ctx, exe, codex_home);
    log::debug!("codex app-server: starting {}", exe.display());
    let mut child = ctx.spawner.spawn(&spec).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            FetchError::Unavailable {
                detail: "codex.exe not found".to_owned(),
            }
        } else {
            log::debug!("codex app-server: spawn failed: {e}");
            transient("could not start codex app-server")
        }
    })?;
    let result = converse(ctx, child.as_mut());
    if let Err(e) = child.kill() {
        log::warn!("codex app-server: kill failed: {e}");
    }
    result
}

/// The whole source: find → spawn → initialize → read → kill, within the timeouts and the
/// fetch deadline.
pub fn fetch(
    ctx: &FetchContext<'_>,
    codex_home: &Path,
    override_exe: Option<&Path>,
) -> Result<CodexUsage, FetchError> {
    let exe = find_codex_exe(ctx, override_exe).ok_or_else(|| FetchError::Unavailable {
        detail: "codex.exe not found".to_owned(),
    })?;
    fetch_with(ctx, &exe, codex_home)
}
