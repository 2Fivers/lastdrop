//! Production implementations of the I/O traits: [`RealClock`], [`RealFs`], [`UreqHttp`] (TLS via
//! SChannel with the system roots, no async runtime), [`RealSpawner`] (`CREATE_NO_WINDOW`) and
//! [`RealEnv`]. The secret store's production implementation is Credential Manager in
//! `lastdrop-win::credentials`, adapted to [`crate::io::Secrets`] by the app.

use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::OwnedHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use jiff::Timestamp;

use crate::io::{
    ChildIo, Clock, DirEntry, Env, FileMeta, Fs, HeaderValue, Http, HttpError, HttpRequest,
    HttpResponse, MAX_BODY_BYTES, Method, ReadAt, SpawnSpec, Spawner, wildcard_match,
};

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// `std::env` and `%USERPROFILE%`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealEnv;

impl Env for RealEnv {
    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

/// The real file system, read-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFs;

/// `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`: another process may read, write,
/// rename and delete the file while [`Fs::open_at`]'s handle is open (std's default too, spelled
/// out because Chromium's cache needs all three).
const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;

/// A file of [`Fs::open_at`]: positioned reads (`ReadFile` with an offset), no shared cursor.
struct RealReadAt {
    file: File,
    size: u64,
}

impl ReadAt for RealReadAt {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_exact_at(&mut self, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match self.file.seek_read(buf, offset) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "file ended before the read",
                    ));
                }
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // ReadFile at or past the end of a file reports ERROR_HANDLE_EOF.
                Err(e) if e.raw_os_error() == Some(38) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "file ended before the read",
                    ));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn meta_of(m: &std::fs::Metadata) -> FileMeta {
    let modified = m.modified().ok().and_then(|t| Timestamp::try_from(t).ok());
    FileMeta {
        len: if m.is_dir() { 0 } else { m.len() },
        modified,
        is_dir: m.is_dir(),
    }
}

impl Fs for RealFs {
    fn read(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len > max_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file larger than allowed",
            ));
        }
        let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        file.by_ref().take(max_len).read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn open(&self, path: &Path, max_len: u64) -> io::Result<Box<dyn Read + Send>> {
        let file = File::open(path)?;
        if file.metadata()?.len() > max_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file larger than allowed",
            ));
        }
        Ok(Box::new(BufReader::new(file).take(max_len)))
    }

    fn read_tail(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let start = len.saturating_sub(max_len);
        file.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::with_capacity(usize::try_from(len - start).unwrap_or(0));
        file.take(max_len).read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn open_at(&self, path: &Path) -> io::Result<Box<dyn ReadAt>> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_ALL)
            .open(path)?;
        let size = file.metadata()?.len();
        Ok(Box::new(RealReadAt { file, size }))
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMeta> {
        Ok(meta_of(&std::fs::metadata(path)?))
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            out.push(DirEntry {
                path: entry.path(),
                meta: meta_of(&entry.metadata()?),
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn glob(&self, dir: &Path, pattern: &str) -> io::Result<Vec<DirEntry>> {
        let mut out = self.list(dir)?;
        out.retain(|e| {
            e.path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| wildcard_match(pattern, n))
        });
        Ok(out)
    }
}

// ---- HTTP ---------------------------------------------------------------------------------------

/// The blocking HTTP client: `ureq` with native-tls (SChannel) and the Windows root store, no
/// redirects (a redirect off the documented endpoint is an error), 4xx/5xx returned as
/// responses, bodies capped at [`crate::io::MAX_BODY_BYTES`].
///
/// Only `https://` URLs are sent. Header values marked [`HeaderValue::Secret`] are sent as
/// sensitive `http` header values and never appear in this type's errors or logs (ureq's own
/// debug log redacts every header outside a short allow-list).
///
/// Needs ureq's `native-tls` feature: with only `native-tls-no-default` ureq compiles no TLS
/// connector and panics on the first `https://` request (an abort in release builds).
/// [`TLS_COMPILED_IN`] records which one the workspace enables; when it is `false`, `https://`
/// requests fail as [`HttpError::Tls`] instead of reaching ureq.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl UreqHttp {
    /// `User-Agent: Lastdrop/<version>`.
    pub fn new() -> UreqHttp {
        UreqHttp::with_proxy(ureq::Proxy::try_from_env())
    }

    /// The client with an explicit proxy (`new` reads the usual `*_PROXY` variables); tests use
    /// `None` so a proxy set on the machine cannot capture their loopback requests.
    fn with_proxy(proxy: Option<ureq::Proxy>) -> UreqHttp {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            // No connection pool: providers poll minutes apart, long after servers drop idle
            // connections, and ureq only purges its pool on the next request, so a pooled TLS
            // connection would hold ~0.5 MB for nothing.
            .max_idle_connections(0)
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(proxy)
            .user_agent(concat!("Lastdrop/", env!("CARGO_PKG_VERSION")))
            .build();
        UreqHttp {
            agent: config.into(),
        }
    }
}

impl Default for UreqHttp {
    fn default() -> UreqHttp {
        UreqHttp::new()
    }
}

impl fmt::Debug for UreqHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UreqHttp")
    }
}

/// Whether ureq was built with its native-tls connector (the workspace's ureq feature
/// `native-tls`, not `native-tls-no-default`).
///
/// With only `native-tls-no-default`, ureq compiles the `native-tls` crate but not its connector,
/// and panics on every `https://` request ("provider is NativeTls but feature is not enabled"),
/// which aborts a release build. If the workspace ever drops back to that feature, set this to
/// `false` and [`UreqHttp`] answers `https://` requests with [`HttpError::Tls`] instead of
/// panicking. The test `tls_switch_matches_ureqs_features` fails when the two disagree.
pub const TLS_COMPILED_IN: bool = true;

/// `https://` only. Unit tests may also talk plain HTTP to a server of their own on the
/// loopback address.
fn scheme_allowed(url: &str) -> bool {
    is_https(url) || (cfg!(test) && url.starts_with("http://127.0.0.1:"))
}

/// The URL starts with `https://` (any case).
fn is_https(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

/// Adds the request's headers; secret values are marked sensitive. Errors name the header,
/// never its value.
fn with_headers<B>(
    mut builder: ureq::RequestBuilder<B>,
    headers: &[(String, HeaderValue)],
) -> Result<ureq::RequestBuilder<B>, HttpError> {
    use ureq::http::{HeaderName, HeaderValue as WireValue};
    for (name, value) in headers {
        let wire_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| HttpError::Other(format!("invalid header name {name:?}")))?;
        let mut wire_value = WireValue::from_str(value.as_str())
            .map_err(|_| HttpError::Other(format!("invalid value for header {name}")))?;
        wire_value.set_sensitive(matches!(value, HeaderValue::Secret(_)));
        builder = builder.header(wire_name, wire_value);
    }
    Ok(builder)
}

/// Classifies a ureq failure. The texts carry no header values (ureq's errors never include
/// them) and no body.
fn map_ureq_error(error: ureq::Error) -> HttpError {
    match error {
        ureq::Error::Timeout(_) => HttpError::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => HttpError::Unreachable,
        ureq::Error::BodyExceedsLimit(_) => HttpError::TooLarge,
        ureq::Error::Tls(text) => HttpError::Tls(text.to_owned()),
        ureq::Error::Io(e) => map_io_error(&e),
        other => {
            let text = other.to_string();
            // native-tls and certificate errors exist only with ureq's `native-tls` feature;
            // recognise them by their prefix so this compiles with any feature set.
            if text.starts_with("native-tls") || text.starts_with("der:") {
                HttpError::Tls(text)
            } else {
                HttpError::Other(text)
            }
        }
    }
}

fn map_io_error(e: &io::Error) -> HttpError {
    use io::ErrorKind as K;
    match e.kind() {
        K::TimedOut | K::WouldBlock => HttpError::Timeout,
        K::ConnectionRefused
        | K::AddrNotAvailable
        | K::NetworkUnreachable
        | K::HostUnreachable
        | K::NetworkDown
        | K::NotConnected => HttpError::Unreachable,
        _ => HttpError::Other(format!("I/O: {e}")),
    }
}

impl Http for UreqHttp {
    fn request(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        if !scheme_allowed(&request.url) {
            return Err(HttpError::Other(
                "only https:// URLs are allowed".to_owned(),
            ));
        }
        if request.timeout.is_zero() {
            return Err(HttpError::Timeout);
        }
        if !TLS_COMPILED_IN && is_https(&request.url) {
            return Err(HttpError::Tls(
                "this build has no TLS support (ureq's native-tls feature is off)".to_owned(),
            ));
        }
        let timeout = Some(request.timeout);
        let url = request.url.as_str();
        let result = match request.method {
            Method::Get => {
                if request.body.is_some() {
                    return Err(HttpError::Other("a GET request has no body".to_owned()));
                }
                let builder = self.agent.get(url).config().timeout_global(timeout).build();
                with_headers(builder, &request.headers)?.call()
            }
            Method::Post => {
                let builder = self
                    .agent
                    .post(url)
                    .config()
                    .timeout_global(timeout)
                    .build();
                let body: &[u8] = request.body.as_deref().unwrap_or_default();
                with_headers(builder, &request.headers)?.send(body)
            }
        };
        let mut response = result.map_err(map_ureq_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(map_ureq_error)?;
        log::debug!("HTTP {:?} {} -> {status}", request.method, request.url);
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

// ---- Processes ----------------------------------------------------------------------------------

/// Starts processes with `CREATE_NO_WINDOW` (0x0800_0000) through
/// `std::os::windows::process::CommandExt`, piped stdin/stdout, stderr to null. A reader thread
/// per child turns stdout into lines so [`ChildIo::read_line`] can time out.
///
/// Every child is put in its own Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: killing
/// the child terminates the job (the child and everything it started), and if Lastdrop exits or
/// crashes the kernel closes the job handle and kills them too. The child joins the job a few
/// microseconds after it is created (std cannot start it suspended); in that window it is still
/// being loaded, so in practice nothing it starts escapes the job. If no job can be created or
/// assigned, the child is still killed on its own ([`Child::kill`]) and a warning is logged.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealSpawner;

/// `CREATE_NO_WINDOW` process creation flag.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Longest stdout line passed on, 1 MiB (as [`MAX_BODY_BYTES`]); a longer line is reported as
/// `ErrorKind::InvalidData` and skipped.
const MAX_LINE_BYTES: usize = 1 << 20;

/// Lines buffered between the reader thread and [`ChildIo::read_line`]; when full, the reader
/// waits, so a chatty child cannot grow our memory.
const LINE_QUEUE: usize = 16;

/// How long [`ChildIo::kill`] waits for the child to exit after it was terminated.
const KILL_WAIT: Duration = Duration::from_secs(2);

/// How long [`ChildIo::kill`] lets the child exit by itself once its input is closed, before it
/// is terminated (**not from the spec**). `codex app-server` renews Codex's own sign-in; a
/// termination between OpenAI replacing the single-use refresh token and Codex saving the new one
/// would sign the user out of Codex (ARCHITECTURE goal 2). At the end of its input the app-server
/// finishes what it is doing and exits on its own.
pub const EXIT_GRACE: Duration = Duration::from_secs(5);

impl Spawner for RealSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        Ok(Box::new(spawn_child(spec)?))
    }
}

/// Starts `spec` in its own kill-on-close job with the stdout reader thread running.
fn spawn_child(spec: &SpawnSpec) -> io::Result<RealChild> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    for (name, value) in &spec.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let label = program_label(&spec.program);

    // The job exists before the child so a failure here cannot leave a child behind.
    let job = match job::kill_on_close() {
        Ok(job) => Some(job),
        Err(e) => {
            log::warn!("{label}: no job object ({e}); the child is killed on its own");
            None
        }
    };
    let mut child = command.spawn()?;
    let job = job.and_then(|job| match job::assign(&job, &child) {
        Ok(()) => Some(job),
        Err(e) => {
            log::warn!("{label}: not in a job object ({e}); the child is killed on its own");
            None
        }
    });

    let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
        // Both are piped above, so this does not happen; never leave the child running.
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("child pipes missing"));
    };
    let (sender, lines) = mpsc::sync_channel(LINE_QUEUE);
    let mut real = RealChild {
        child,
        stdin: Some(stdin),
        lines,
        job,
        killed: false,
        label,
        grace: EXIT_GRACE,
    };
    let reader = std::thread::Builder::new()
        .name("lastdrop-child-stdout".to_owned())
        .spawn(move || read_lines(stdout, &sender));
    if let Err(e) = reader {
        let _ = real.kill();
        return Err(e);
    }
    log::debug!("{}: started", real.label);
    Ok(real)
}

/// The program's file name, for logs (no arguments, no folders).
fn program_label(program: &Path) -> String {
    program
        .file_name()
        .map_or_else(|| "child".to_owned(), |n| n.to_string_lossy().into_owned())
}

/// A running child with piped stdin/stdout. Dropping it kills the child.
struct RealChild {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<io::Result<String>>,
    job: Option<OwnedHandle>,
    killed: bool,
    label: String,
    /// [`EXIT_GRACE`] (shorter in tests).
    grace: Duration,
}

impl ChildIo for RealChild {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        use std::io::Write as _;
        if line.contains(['\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a line must not contain a line break",
            ));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "child killed"))?;
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        stdin.write_all(&buf)?;
        stdin.flush()
    }

    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        if self.killed {
            return Ok(None);
        }
        match self.lines.recv_timeout(timeout) {
            Ok(Ok(line)) => Ok(Some(line)),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "no complete line from the child in time",
            )),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        if self.killed {
            return Ok(());
        }
        self.killed = true;
        // Closing stdin lets the child see the end of its input and exit by itself; it is
        // terminated only if it is still running after the grace period ([`EXIT_GRACE`]).
        self.stdin = None;
        if job::wait(&self.child, self.grace).unwrap_or(false) {
            let _ = self.child.try_wait();
            // The child itself is done; anything it left running (its own children, its
            // console host) ends with the job.
            if let Some(job) = self.job.take()
                && let Err(e) = job::terminate(&job)
            {
                log::debug!("{}: leftover processes not terminated: {e}", self.label);
            }
            log::debug!("{}: exited", self.label);
            return Ok(());
        }
        log::debug!(
            "{}: still running after {:?}; terminating",
            self.label,
            self.grace
        );
        let via_job = self
            .job
            .as_ref()
            .is_some_and(|job| job::terminate(job).is_ok());
        if !via_job {
            match self.child.kill() {
                Ok(()) => {}
                // Already exited.
                Err(e) if e.kind() == io::ErrorKind::InvalidInput => {}
                Err(e) => return Err(e),
            }
        }
        let exited = job::wait(&self.child, KILL_WAIT)?;
        // Collects the exit status; the process object is released when `child` drops.
        let _ = self.child.try_wait();
        // Closing the job kills anything of the child's that is still running.
        self.job = None;
        if exited {
            log::debug!("{}: stopped", self.label);
            Ok(())
        } else {
            log::warn!("{}: did not exit within {KILL_WAIT:?}", self.label);
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child did not exit after kill",
            ))
        }
    }
}

impl Drop for RealChild {
    fn drop(&mut self) {
        if let Err(e) = self.kill() {
            log::warn!("{}: kill on drop failed: {e}", self.label);
        }
    }
}

/// The reader thread: stdout → complete lines (without `\r\n`) → channel. Ends at end of stream,
/// on a read error (sent on) or when the receiver is gone.
fn read_lines(stdout: ChildStdout, sender: &SyncSender<io::Result<String>>) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        let item = match read_capped_line(&mut reader, &mut buf, MAX_LINE_BYTES) {
            Ok(LineRead::Eof) => return,
            Ok(LineRead::Line) => {
                while buf.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
                    buf.pop();
                }
                Ok(String::from_utf8_lossy(&buf).into_owned())
            }
            Ok(LineRead::TooLong) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "child output line longer than 1 MiB",
            )),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = sender.send(Err(e));
                return;
            }
        };
        if sender.send(item).is_err() {
            return;
        }
    }
}

/// What [`read_capped_line`] found.
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// A line in `buf` (with its `\n`, or the last bytes before end of stream).
    Line,
    /// A line longer than the cap; its bytes were skipped up to and including the `\n`.
    TooLong,
    /// End of stream, nothing read.
    Eof,
}

/// Reads one line into `buf` (cleared first) without ever holding more than `cap` bytes of it.
fn read_capped_line(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    cap: usize,
) -> io::Result<LineRead> {
    buf.clear();
    let mut too_long = false;
    let mut read_any = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(match (read_any, too_long) {
                (false, _) => LineRead::Eof,
                (true, true) => LineRead::TooLong,
                (true, false) => LineRead::Line,
            });
        }
        read_any = true;
        let (chunk, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (&available[..=i], true),
            None => (available, false),
        };
        let used = chunk.len();
        if !too_long {
            if buf.len() + chunk.len() > cap {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        reader.consume(used);
        if done {
            return Ok(if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
    }
}

/// Default timeout of one HTTP call when the provider does not say (**not from the spec**).
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// The few kernel32 calls for Job Objects. Declared here because `lastdrop-providers` has no
/// `windows` dependency (docs/ARCHITECTURE.md: Win32 lives in `lastdrop-win`; this is process
/// management, not windows).
mod job {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::process::Child;
    use std::time::Duration;

    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    /// `JobObjectExtendedLimitInformation` (JOBOBJECTINFOCLASS).
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    /// `JobObjectBasicAccountingInformation` (JOBOBJECTINFOCLASS).
    #[cfg(test)]
    const JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION: i32 = 1;
    /// `WAIT_OBJECT_0`.
    const WAIT_OBJECT_0: u32 = 0;
    /// `WAIT_TIMEOUT`.
    const WAIT_TIMEOUT: u32 = 0x0000_0102;

    /// `JOBOBJECT_BASIC_LIMIT_INFORMATION`.
    #[repr(C)]
    #[derive(Default)]
    pub(super) struct BasicLimitInformation {
        pub per_process_user_time_limit: i64,
        pub per_job_user_time_limit: i64,
        pub limit_flags: u32,
        pub minimum_working_set_size: usize,
        pub maximum_working_set_size: usize,
        pub active_process_limit: u32,
        pub affinity: usize,
        pub priority_class: u32,
        pub scheduling_class: u32,
    }

    /// `IO_COUNTERS`.
    #[repr(C)]
    #[derive(Default)]
    pub(super) struct IoCounters {
        pub read_operation_count: u64,
        pub write_operation_count: u64,
        pub other_operation_count: u64,
        pub read_transfer_count: u64,
        pub write_transfer_count: u64,
        pub other_transfer_count: u64,
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`.
    #[repr(C)]
    #[derive(Default)]
    pub(super) struct ExtendedLimitInformation {
        pub basic_limit_information: BasicLimitInformation,
        pub io_info: IoCounters,
        pub process_memory_limit: usize,
        pub job_memory_limit: usize,
        pub peak_process_memory_used: usize,
        pub peak_job_memory_used: usize,
    }

    /// `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`.
    #[cfg(test)]
    #[repr(C)]
    #[derive(Default)]
    pub(super) struct BasicAccountingInformation {
        pub total_user_time: i64,
        pub total_kernel_time: i64,
        pub this_period_total_user_time: i64,
        pub this_period_total_kernel_time: i64,
        pub total_page_fault_count: u32,
        pub total_processes: u32,
        pub active_processes: u32,
        pub total_terminated_processes: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> RawHandle;
        fn SetInformationJobObject(
            job: RawHandle,
            class: i32,
            info: *const c_void,
            info_len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: RawHandle, process: RawHandle) -> i32;
        fn TerminateJobObject(job: RawHandle, exit_code: u32) -> i32;
        fn WaitForSingleObject(handle: RawHandle, milliseconds: u32) -> u32;
        #[cfg(test)]
        fn QueryInformationJobObject(
            job: RawHandle,
            class: i32,
            info: *mut c_void,
            info_len: u32,
            return_len: *mut u32,
        ) -> i32;
    }

    fn check(ok: i32) -> io::Result<()> {
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// A new anonymous job whose processes are killed when its last handle closes.
    pub(super) fn kill_on_close() -> io::Result<OwnedHandle> {
        // SAFETY: both pointers may be null (default security, no name); the call has no other
        // preconditions.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a valid job handle that we own exclusively from here on.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut info = ExtendedLimitInformation::default();
        info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let len = u32::try_from(size_of::<ExtendedLimitInformation>())
            .map_err(|_| io::Error::other("job information too large"))?;
        // SAFETY: `job` is a live job handle; `info` is a properly laid out
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION of `len` bytes that outlives the call.
        check(unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&raw const info).cast(),
                len,
            )
        })?;
        Ok(job)
    }

    /// Puts `child` in `job`.
    pub(super) fn assign(job: &OwnedHandle, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are live for the duration of the call (borrowed from owners).
        check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) })
    }

    /// Terminates every process in `job`.
    pub(super) fn terminate(job: &OwnedHandle) -> io::Result<()> {
        // SAFETY: `job` is a live job handle borrowed from its owner.
        check(unsafe { TerminateJobObject(job.as_raw_handle(), 1) })
    }

    /// Waits up to `timeout` for `child` to exit; `Ok(true)` when it has.
    pub(super) fn wait(child: &Child, timeout: Duration) -> io::Result<bool> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the process handle is live while `child` is borrowed.
        match unsafe { WaitForSingleObject(child.as_raw_handle(), ms) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// The number of processes alive in `job` (tests).
    #[cfg(test)]
    pub(super) fn active_processes(job: &OwnedHandle) -> io::Result<u32> {
        let mut info = BasicAccountingInformation::default();
        let len = u32::try_from(size_of::<BasicAccountingInformation>())
            .map_err(|_| io::Error::other("job information too large"))?;
        // SAFETY: `job` is live; `info` is a writable JOBOBJECT_BASIC_ACCOUNTING_INFORMATION of
        // `len` bytes; the returned-length pointer may be null.
        check(unsafe {
            QueryInformationJobObject(
                job.as_raw_handle(),
                JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                (&raw mut info).cast(),
                len,
                std::ptr::null_mut(),
            )
        })?;
        Ok(info.active_processes)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use lastdrop_core::secret::SecretString;

    use super::{
        LineRead, RealSpawner, TLS_COMPILED_IN, UreqHttp, job, read_capped_line, spawn_child,
    };
    use crate::io::{
        ChildIo, HeaderValue, Http, HttpError, HttpRequest, MAX_BODY_BYTES, Method, SpawnSpec,
        Spawner,
    };

    // ---- HTTP against a loopback server of the test's own ---------------------------------------

    /// What the loopback server received.
    struct Received {
        head: String,
        body: Vec<u8>,
    }

    /// Serves one connection: reads the request, then writes `response` (unless `None`, which
    /// holds the connection open for `hold` without answering).
    fn serve_once(
        response: Option<Vec<u8>>,
        hold: Duration,
    ) -> (String, JoinHandle<Option<Received>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().ok()?;
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
            let received = read_request(&stream)?;
            let mut stream = stream;
            match response {
                Some(bytes) => {
                    stream.write_all(&bytes).ok()?;
                    stream.flush().ok()?;
                }
                None => std::thread::sleep(hold),
            }
            Some(received)
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn read_request(stream: &TcpStream) -> Option<Received> {
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            if line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        let len = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let mut body = vec![0; len];
        reader.read_exact(&mut body).ok()?;
        Some(Received { head, body })
    }

    fn client() -> UreqHttp {
        UreqHttp::with_proxy(None)
    }

    fn get(url: String, headers: Vec<(String, HeaderValue)>) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url,
            headers,
            body: None,
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn returns_status_headers_and_body_and_sends_headers() {
        let (base, server) = serve_once(
            Some(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nRetry-After: 7\r\n\
                  Content-Length: 15\r\n\r\n{\"balance\": 10}"
                    .to_vec(),
            ),
            Duration::ZERO,
        );
        let secret = SecretString::new("Bearer test-token-not-real".to_owned());
        let request = get(
            format!("{base}/openapi/v1/balance"),
            vec![
                ("Authorization".to_owned(), HeaderValue::Secret(secret)),
                (
                    "Accept".to_owned(),
                    HeaderValue::Plain("application/json".to_owned()),
                ),
            ],
        );
        let response = client().request(&request).expect("response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"balance": 10}"#);
        assert_eq!(response.header("retry-after"), Some("7"));
        assert_eq!(response.header("CONTENT-TYPE"), Some("application/json"));
        let received = server.join().expect("server").expect("request");
        let head = received.head.to_ascii_lowercase();
        assert!(head.starts_with("get /openapi/v1/balance http/1.1"));
        assert!(head.contains("authorization: bearer test-token-not-real"));
        assert!(head.contains("accept: application/json"));
        assert!(head.contains(&format!(
            "user-agent: lastdrop/{}",
            env!("CARGO_PKG_VERSION")
        )));
    }

    #[test]
    fn error_statuses_and_redirects_are_responses() {
        for (raw, status) in [
            (
                &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\n{}"[..],
                401,
            ),
            (
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
                503,
            ),
            (
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\n\
                  Content-Length: 0\r\n\r\n",
                302,
            ),
        ] {
            let (base, server) = serve_once(Some(raw.to_vec()), Duration::ZERO);
            let response = client()
                .request(&get(format!("{base}/x"), Vec::new()))
                .expect("a response, not an error");
            assert_eq!(response.status, status);
            server.join().expect("server").expect("request");
        }
    }

    #[test]
    fn posts_the_body() {
        let (base, server) = serve_once(
            Some(b"HTTP/1.1 204 No Content\r\n\r\n".to_vec()),
            Duration::ZERO,
        );
        let request = HttpRequest {
            method: Method::Post,
            url: format!("{base}/rpc"),
            headers: vec![(
                "Content-Type".to_owned(),
                HeaderValue::Plain("application/json".to_owned()),
            )],
            body: Some(br#"{"id":1}"#.to_vec()),
            timeout: Duration::from_secs(5),
        };
        let response = client().request(&request).expect("response");
        assert_eq!(response.status, 204);
        let received = server.join().expect("server").expect("request");
        assert!(received.head.starts_with("POST /rpc HTTP/1.1"));
        assert_eq!(received.body, br#"{"id":1}"#);
    }

    #[test]
    fn a_body_over_the_cap_is_too_large() {
        let len = usize::try_from(MAX_BODY_BYTES).expect("fits") + 1;
        let mut raw = format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n").into_bytes();
        raw.resize(raw.len() + len, b'x');
        let (base, server) = serve_once(Some(raw), Duration::ZERO);
        let err = client()
            .request(&get(format!("{base}/big"), Vec::new()))
            .expect_err("too large");
        assert_eq!(err, HttpError::TooLarge);
        let _ = server.join();
    }

    #[test]
    fn a_silent_server_times_out_within_the_request_timeout() {
        let (base, server) = serve_once(None, Duration::from_secs(1));
        let mut request = get(format!("{base}/slow"), Vec::new());
        request.timeout = Duration::from_millis(300);
        let started = Instant::now();
        let err = client().request(&request).expect_err("timeout");
        assert_eq!(err, HttpError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(2));
        let _ = server.join();
    }

    #[test]
    fn a_refused_connection_is_unreachable() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let err = client()
            .request(&get(format!("http://127.0.0.1:{port}/"), Vec::new()))
            .expect_err("refused");
        assert_eq!(err, HttpError::Unreachable);
    }

    #[test]
    fn only_https_is_sent_and_a_zero_timeout_sends_nothing() {
        for url in [
            "http://example.invalid/",
            "ftp://example.invalid/",
            "balance",
        ] {
            let err = client()
                .request(&get(url.to_owned(), Vec::new()))
                .expect_err("refused scheme");
            assert!(matches!(err, HttpError::Other(_)), "{url}: {err:?}");
        }
        let mut request = get("https://example.invalid/".to_owned(), Vec::new());
        request.timeout = Duration::ZERO;
        assert_eq!(client().request(&request), Err(HttpError::Timeout));
    }

    #[test]
    fn invalid_headers_are_errors_that_do_not_show_the_value() {
        let secret = SecretString::new("bad\nvalue-not-real".to_owned());
        let request = get(
            "http://127.0.0.1:9/".to_owned(),
            vec![("Authorization".to_owned(), HeaderValue::Secret(secret))],
        );
        let err = client().request(&request).expect_err("invalid value");
        let text = format!("{err} {err:?}");
        assert!(text.contains("Authorization"));
        assert!(!text.contains("value-not-real"));
    }

    /// A loopback server that accepts one connection and answers in plain text, so a TLS
    /// handshake against it fails. Returns its port and a function that unblocks and joins it
    /// (it may never have been connected to).
    fn plain_text_server() -> (u16, impl FnOnce()) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });
        let finish = move || {
            // Unblocks `accept` when the client never connected; refused when it did.
            let _ = TcpStream::connect(("127.0.0.1", port));
            let _ = server.join();
        };
        (port, finish)
    }

    /// The TLS path, against a loopback server that answers in plain text: the request must fail
    /// as an error, never panic (without the TLS connector it fails before connecting).
    #[test]
    fn https_fails_cleanly_against_a_server_without_tls() {
        let (port, finish) = plain_text_server();
        let mut request = get(format!("https://127.0.0.1:{port}/"), Vec::new());
        request.timeout = Duration::from_secs(3);
        let err = client()
            .request(&request)
            .expect_err("no TLS on the server");
        assert!(
            matches!(err, HttpError::Tls(_) | HttpError::Other(_)),
            "{err:?}"
        );
        finish();
    }

    /// [`TLS_COMPILED_IN`] agrees with how ureq was built: ureq without its TLS connector panics
    /// on an `https://` request; with it, the request reaches the server and fails as an error.
    #[test]
    fn tls_switch_matches_ureqs_features() {
        let (port, finish) = plain_text_server();
        let agent = UreqHttp::with_proxy(None).agent;
        let url = format!("https://127.0.0.1:{port}/");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            agent
                .get(&url)
                .config()
                .timeout_global(Some(Duration::from_secs(3)))
                .build()
                .call()
                .map(|_| ())
        }));
        finish();
        assert_eq!(
            outcome.is_ok(),
            TLS_COMPILED_IN,
            "TLS_COMPILED_IN must match ureq's native-tls feature in the workspace Cargo.toml"
        );
    }

    // ---- Lines ----------------------------------------------------------------------------------

    #[test]
    fn capped_lines() {
        let mut input: &[u8] = b"one\r\ntwo\nthree-is-long\nfour";
        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut input, &mut buf, 8).ok(),
            Some(LineRead::Line)
        );
        assert_eq!(buf, b"one\r\n");
        assert_eq!(
            read_capped_line(&mut input, &mut buf, 8).ok(),
            Some(LineRead::Line)
        );
        assert_eq!(buf, b"two\n");
        assert_eq!(
            read_capped_line(&mut input, &mut buf, 8).ok(),
            Some(LineRead::TooLong)
        );
        assert_eq!(
            read_capped_line(&mut input, &mut buf, 8).ok(),
            Some(LineRead::Line)
        );
        assert_eq!(buf, b"four");
        assert_eq!(
            read_capped_line(&mut input, &mut buf, 8).ok(),
            Some(LineRead::Eof)
        );
    }

    // ---- Processes ------------------------------------------------------------------------------

    fn cmd_exe() -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        PathBuf::from(root).join("System32").join("cmd.exe")
    }

    fn spec(args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            program: cmd_exe(),
            args: args.iter().map(OsString::from).collect(),
            env: Vec::new(),
            cwd: None,
        }
    }

    fn spawn_boxed(spec: &SpawnSpec) -> Box<dyn ChildIo> {
        RealSpawner.spawn(spec).expect("spawn cmd.exe")
    }

    #[test]
    fn a_batch_script_gets_its_arguments_intact_from_a_path_with_spaces_and_ampersands() {
        // npm's `codex.cmd` is spawned directly (`codex::app_server::spawn_spec`): std runs it
        // through cmd.exe and quotes for it, so `&` in the profile folder's name does not end
        // the command, and arguments arrive as given, `%…%` not expanded.
        let dir = std::env::temp_dir().join(format!(
            "lastdrop spawn A&B ^! {}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let script = dir.join("echo args.cmd");
        std::fs::write(&script, "@echo off\r\necho(%*\r\necho(%~3\r\n").expect("script");
        let spec = SpawnSpec {
            program: script,
            args: ["app-server", "-c", "features.plugins=false", "a&b %PATH% c"]
                .iter()
                .map(OsString::from)
                .collect(),
            env: Vec::new(),
            cwd: Some(dir.clone()),
        };
        let mut child = spawn_boxed(&spec);
        let all = child.read_line(Duration::from_secs(10)).expect("line");
        let third = child.read_line(Duration::from_secs(10)).expect("line");
        child.kill().expect("kill");
        let _ = std::fs::remove_dir_all(&dir);
        let all = all.expect("the script ran");
        assert!(all.starts_with("app-server -c "), "{all}");
        assert!(all.ends_with(r#" "a&b %PATH% c""#), "{all}");
        assert_eq!(third.as_deref(), Some("features.plugins=false"));
    }

    #[test]
    fn a_job_kills_the_whole_tree() {
        // The outer cmd starts an inner cmd that waits for input: two processes in the job.
        // (conhost.exe may be counted too.)
        let mut child = spawn_child(&spec(&["/c", "cmd /c set /p WAIT="])).expect("spawn");
        let job = child.job.as_ref().expect("in a job");
        let deadline = Instant::now() + Duration::from_secs(5);
        while job::active_processes(job).expect("query") < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(job::active_processes(job).expect("query") >= 2);
        let job_copy = job.try_clone().expect("clone job handle");
        child.kill().expect("kill");
        assert_eq!(job::active_processes(&job_copy).expect("query"), 0);
        assert!(child.child.try_wait().expect("status").is_some());
        // Idempotent.
        child.kill().expect("second kill");
    }

    #[test]
    fn kill_lets_the_child_exit_by_itself_before_terminating_it() {
        // Exits 0 at the end of its input: not terminated (TerminateJobObject would give 1).
        let mut child = spawn_child(&spec(&["/c", "set /p WAIT=&exit 0"])).expect("spawn");
        child.kill().expect("kill");
        let status = child.child.try_wait().expect("status").expect("exited");
        assert_eq!(status.code(), Some(0), "exited by itself");

        // Ignores its input: terminated after the grace period. `WaitForSingleObject` counts its
        // timeout in system clock ticks (15.6 ms by default) from a tick that has already begun,
        // so as `Instant` measures it the wait can end up to a tick early: allow two.
        let mut busy = spawn_child(&spec(&["/c", "for /l %i in (0,0,1) do @rem"])).expect("spawn");
        busy.grace = Duration::from_millis(200);
        let started = Instant::now();
        busy.kill().expect("kill");
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(200 - 32), "{waited:?}");
        let status = busy.child.try_wait().expect("status").expect("exited");
        assert_eq!(status.code(), Some(1), "terminated");
    }

    #[test]
    fn dropping_the_child_kills_it() {
        let child = spawn_child(&spec(&["/c", "set /p WAIT="])).expect("spawn");
        let job = child
            .job
            .as_ref()
            .expect("in a job")
            .try_clone()
            .expect("clone job handle");
        drop(child);
        assert_eq!(job::active_processes(&job).expect("query"), 0);
    }

    #[test]
    fn the_job_layout_matches_windows() {
        // x64 sizes from winnt.h.
        assert_eq!(size_of::<job::ExtendedLimitInformation>(), 144);
        assert_eq!(size_of::<job::BasicAccountingInformation>(), 48);
    }

    #[test]
    fn a_line_waits_for_a_write_and_times_out_without_one() {
        let mut child = spawn_boxed(&spec(&["/v:on", "/c", "set /p LINE=&echo got:!LINE!"]));
        let err = child
            .read_line(Duration::from_millis(200))
            .expect_err("nothing written yet");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        child.write_line("ping-1").expect("write");
        assert_eq!(
            child.read_line(Duration::from_secs(10)).expect("line"),
            Some("got:ping-1".to_owned())
        );
        assert_eq!(child.read_line(Duration::from_secs(10)).expect("eof"), None);
        assert!(child.write_line("two\nlines").is_err());
        child.kill().expect("kill");
    }
}
