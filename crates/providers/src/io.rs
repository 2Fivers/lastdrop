//! The injectable I/O traits of a fetch: [`Clock`], [`Http`], [`Fs`], [`Secrets`], [`Spawner`]
//! (with [`ChildIo`]) and [`Env`]. Production implementations are in [`crate::real`], test
//! implementations in [`crate::fakes`].

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{AccountKey, ProviderId};
use lastdrop_core::secret::SecretString;

// ---- Clock ----------------------------------------------------------------------------------------

/// The wall clock (UTC).
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> Timestamp;
}

// ---- HTTP -----------------------------------------------------------------------------------------

/// Largest response body read, 1 MiB. Usage responses are a few KiB; anything larger is an error.
pub const MAX_BODY_BYTES: u64 = 1 << 20;

/// HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// GET.
    Get,
    /// POST.
    Post,
}

/// A header value; secrets print as `[redacted]` in `Debug` and logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderValue {
    /// A value that may be logged.
    Plain(String),
    /// A token or key (`Authorization: Bearer …`).
    Secret(SecretString),
}

impl HeaderValue {
    /// The value to send.
    pub fn as_str(&self) -> &str {
        match self {
            HeaderValue::Plain(v) => v,
            HeaderValue::Secret(s) => s.expose(),
        }
    }
}

/// An HTTP request. Only to the documented endpoints of the service being read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    /// Method.
    pub method: Method,
    /// Absolute `https://` URL.
    pub url: String,
    /// Headers in order.
    pub headers: Vec<(String, HeaderValue)>,
    /// Request body.
    pub body: Option<Vec<u8>>,
    /// Whole-request timeout (connect + response), capped by the fetch deadline.
    pub timeout: Duration,
}

/// An HTTP response (any status: 4xx/5xx are responses, not errors).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Headers in order.
    pub headers: Vec<(String, String)>,
    /// Body, at most [`MAX_BODY_BYTES`].
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response with `status` and a UTF-8 body and no headers (tests).
    pub fn new(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// The first header named `name` (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `Retry-After` as a duration from `now`: delta-seconds or an HTTP-date (RFC 9110 §10.2.3);
    /// `None` when absent or unreadable.
    #[expect(unused_variables, reason = "M1 contract stub")]
    pub fn retry_after(&self, now: Timestamp) -> Option<Duration> {
        todo!("M1: parse Retry-After (seconds or HTTP-date)")
    }
}

/// A transport-level HTTP failure (no response).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// DNS failure or connection refused/unreachable: counts as OFFLINE (R§3.8).
    #[error("cannot reach the server")]
    Unreachable,
    /// The request did not complete within its timeout.
    #[error("timed out")]
    Timeout,
    /// TLS handshake or certificate failure.
    #[error("TLS: {0}")]
    Tls(String),
    /// The body exceeded [`MAX_BODY_BYTES`].
    #[error("response too large")]
    TooLarge,
    /// Anything else (no secrets in the text).
    #[error("{0}")]
    Other(String),
}

/// A blocking HTTP client.
pub trait Http: Send + Sync {
    /// Sends `request` and reads the whole response.
    fn request(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

// ---- Files ----------------------------------------------------------------------------------------

/// File metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta {
    /// Size in bytes (0 for directories).
    pub len: u64,
    /// Last modification time, when known.
    pub modified: Option<Timestamp>,
    /// A directory.
    pub is_dir: bool,
}

/// One entry of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// Full path.
    pub path: PathBuf,
    /// Its metadata.
    pub meta: FileMeta,
}

/// Read-only file access: Lastdrop never writes another tool's files.
pub trait Fs: Send + Sync {
    /// The whole file; `ErrorKind::InvalidData` when it is longer than `max_len` (Cursor's
    /// `state.vscdb` can be 1.6 GB: never read such a file whole).
    fn read(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>>;

    /// A buffered reader over the file, for parsing large files as a stream
    /// (`serde_json::from_reader` into a struct that skips everything else with
    /// `serde::de::IgnoredAny`): Claude Code's `.claude.json` grows with project history to
    /// several MB, and reading it whole would break the 10 MB working-set budget (ARCHITECTURE
    /// goal 1). `ErrorKind::InvalidData` when the file is longer than `max_len`.
    fn open(&self, path: &Path, max_len: u64) -> io::Result<Box<dyn io::Read + Send>>;

    /// The last `max_len` bytes of the file (all of it when shorter), for JSONL logs whose newest
    /// event is at the end (Codex rollouts).
    fn read_tail(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>>;

    /// Metadata of a file or directory.
    fn metadata(&self, path: &Path) -> io::Result<FileMeta>;

    /// The entries of `dir` (not recursive), sorted by path.
    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;

    /// The entries of `dir` whose file name matches `pattern` (`*` = any run of characters,
    /// `?` = one character, case-insensitive like Windows), sorted by path.
    fn glob(&self, dir: &Path, pattern: &str) -> io::Result<Vec<DirEntry>>;
}

/// `*`/`?` wildcard match of a file name, ASCII case-insensitive (used by [`Fs::glob`]
/// implementations).
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();
    let n: Vec<char> = name.chars().map(|c| c.to_ascii_lowercase()).collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

// ---- Secrets --------------------------------------------------------------------------------------

/// The name of one of Lastdrop's own secrets: Credential Manager generic credential
/// `Lastdrop:<provider>:<account>:<name>` (ARCHITECTURE "Secrets").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CredentialName {
    /// The service.
    pub provider: ProviderId,
    /// The account.
    pub account: AccountKey,
    /// The secret's name within the account (`api_key`).
    pub name: String,
}

impl CredentialName {
    /// The Credential Manager target name, `Lastdrop:meshy:default:api_key`.
    pub fn target(&self) -> String {
        format!(
            "Lastdrop:{}:{}:{}",
            self.provider.as_str(),
            self.account.0,
            self.name
        )
    }
}

/// Failure of the secret store.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    /// The store refused or failed (Win32 error text, no secret).
    #[error("credential store: {0}")]
    Store(String),
}

/// Lastdrop's own secrets (generic credentials). Never another tool's credentials.
pub trait Secrets: Send + Sync {
    /// The secret, `None` when it does not exist.
    fn get(&self, name: &CredentialName) -> Result<Option<SecretString>, SecretsError>;
    /// Creates or replaces the secret.
    fn set(&self, name: &CredentialName, secret: &SecretString) -> Result<(), SecretsError>;
    /// Deletes the secret; not an error when it does not exist.
    fn delete(&self, name: &CredentialName) -> Result<(), SecretsError>;
}

// ---- Processes ------------------------------------------------------------------------------------

/// A child process to start: always with piped stdin/stdout, stderr discarded (it may echo
/// secrets), `CREATE_NO_WINDOW`, never a console window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    /// The executable (absolute path).
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<OsString>,
    /// Environment overrides on top of Lastdrop's environment: `Some` sets, `None` removes
    /// (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`).
    pub env: Vec<(OsString, Option<OsString>)>,
    /// Working directory, if not inherited.
    pub cwd: Option<PathBuf>,
}

/// The pipes of a running child, line-oriented (JSON-RPC over stdio for `codex app-server`).
/// Dropping it kills the child.
pub trait ChildIo: Send {
    /// Writes `line` and a `\n` to the child's stdin and flushes.
    fn write_line(&mut self, line: &str) -> io::Result<()>;

    /// The next line from stdout without its line ending; `Ok(None)` at end of stream;
    /// `ErrorKind::TimedOut` when nothing complete arrives within `timeout`.
    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>>;

    /// Kills the child (and waits for it, briefly). Idempotent.
    fn kill(&mut self) -> io::Result<()>;
}

/// Starts child processes.
pub trait Spawner: Send + Sync {
    /// Starts `spec`; `ErrorKind::NotFound` when the program does not exist.
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>>;
}

/// A command the **user** runs in a visible console window: the vendor's own sign-in (F§4.3
/// `Open sign-in…`: `claude` / `codex login` with the account's folder set). Never started
/// through [`Spawner`] (no window, piped I/O); the app starts it with
/// `lastdrop_win::shell::open_console`. Lastdrop never reads its output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCommand {
    /// The program, resolved on `PATH` by the console (`claude`, `codex`); an absolute path when
    /// the provider knows one (the `codex.exe` it found).
    pub program: OsString,
    /// Arguments (`login`).
    pub args: Vec<OsString>,
    /// Environment overrides: `Some` sets, `None` removes (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`).
    pub env: Vec<(OsString, Option<OsString>)>,
    /// Working directory, if not inherited.
    pub cwd: Option<PathBuf>,
}

// ---- Environment ----------------------------------------------------------------------------------

/// Environment variables and well-known folders.
pub trait Env: Send + Sync {
    /// An environment variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `APPDATA`, `PATH`).
    fn var(&self, name: &str) -> Option<OsString>;
    /// The user's profile folder (`%USERPROFILE%`).
    fn home_dir(&self) -> Option<PathBuf>;
}

#[cfg(test)]
mod tests {
    use super::wildcard_match;

    #[test]
    fn wildcards() {
        assert!(wildcard_match(
            "rollout-*.jsonl",
            "rollout-2026-10-10T12-00-00-abc.jsonl"
        ));
        assert!(wildcard_match("ROLLOUT-*.JSONL", "rollout-1.jsonl"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("rollout-*.jsonl", "rollout-1.json"));
        assert!(wildcard_match("*", ""));
        assert!(!wildcard_match("?", ""));
    }
}
