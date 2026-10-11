//! Test implementations of the I/O traits, fully working, for every provider test:
//! [`FakeClock`], [`FakeHttp`] (scripted responses, records requests), [`FakeFs`] (in memory),
//! [`FakeSecrets`], [`FakeSpawner`] (scripted child output, records input) and [`FakeEnv`].
//!
//! All data given to these fakes in tests is invented (`tests/fixtures/<provider>/README.md`):
//! never a real token, log or usage file. [`chromium_cache`] builds invented Chromium HTTP caches
//! into a [`FakeFs`].

pub mod chromium_cache;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::secret::SecretString;

use crate::FetchContext;
use crate::io::{
    ChildIo, Clock, CredentialName, DirEntry, Env, FileMeta, Fs, Http, HttpError, HttpRequest,
    HttpResponse, Method, ReadAt, Secrets, SecretsError, SpawnSpec, Spawner, wildcard_match,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- Clock ----------------------------------------------------------------------------------------

/// A clock that only moves when told.
#[derive(Debug)]
pub struct FakeClock {
    now: Mutex<Timestamp>,
}

impl FakeClock {
    /// A clock at `now`.
    pub fn new(now: Timestamp) -> FakeClock {
        FakeClock {
            now: Mutex::new(now),
        }
    }

    /// Sets the time.
    pub fn set(&self, now: Timestamp) {
        *lock(&self.now) = now;
    }

    /// Moves the time forward (or back, with a negative duration).
    pub fn advance(&self, by: SignedDuration) {
        let mut now = lock(&self.now);
        *now = now
            .checked_add(by)
            .expect("FakeClock::advance out of range");
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        *lock(&self.now)
    }
}

// ---- HTTP -----------------------------------------------------------------------------------------

/// One scripted answer: method, URL, result.
type Scripted = (Method, String, Result<HttpResponse, HttpError>);

/// An HTTP client that answers from a script and records every request.
///
/// Each scripted answer matches one request by method and exact URL and is used once, in the
/// order scripted. A request without a matching answer gets `HttpError::Other` (so a provider
/// that calls an unexpected endpoint fails its test).
#[derive(Debug, Default)]
pub struct FakeHttp {
    script: Mutex<VecDeque<Scripted>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl FakeHttp {
    /// An empty script.
    pub fn new() -> FakeHttp {
        FakeHttp::default()
    }

    /// Answers the next `method url` request with `response`.
    pub fn respond(&self, method: Method, url: &str, response: HttpResponse) -> &FakeHttp {
        lock(&self.script).push_back((method, url.to_owned(), Ok(response)));
        self
    }

    /// Fails the next `method url` request with `error`.
    pub fn fail(&self, method: Method, url: &str, error: HttpError) -> &FakeHttp {
        lock(&self.script).push_back((method, url.to_owned(), Err(error)));
        self
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        lock(&self.requests).clone()
    }

    /// Scripted answers not used yet (a test can assert this is 0).
    pub fn pending(&self) -> usize {
        lock(&self.script).len()
    }
}

impl Http for FakeHttp {
    fn request(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        lock(&self.requests).push(request.clone());
        let mut script = lock(&self.script);
        let pos = script
            .iter()
            .position(|(m, u, _)| *m == request.method && *u == request.url);
        match pos.and_then(|i| script.remove(i)) {
            Some((_, _, answer)) => answer,
            None => Err(HttpError::Other(format!(
                "FakeHttp: no scripted answer for {:?} {}",
                request.method, request.url
            ))),
        }
    }
}

// ---- Files ----------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct FakeFile {
    data: Vec<u8>,
    modified: Option<Timestamp>,
}

/// An in-memory, read-only (to the code under test) file system. Directories exist implicitly
/// as the parents of files.
#[derive(Debug, Default)]
pub struct FakeFs {
    files: Mutex<BTreeMap<PathBuf, FakeFile>>,
}

impl FakeFs {
    /// An empty file system.
    pub fn new() -> FakeFs {
        FakeFs::default()
    }

    /// Adds or replaces a file.
    pub fn add_file(
        &self,
        path: impl Into<PathBuf>,
        data: impl Into<Vec<u8>>,
        modified: Timestamp,
    ) -> &FakeFs {
        lock(&self.files).insert(
            path.into(),
            FakeFile {
                data: data.into(),
                modified: Some(modified),
            },
        );
        self
    }

    /// Removes a file.
    pub fn remove(&self, path: &Path) {
        lock(&self.files).remove(path);
    }

    fn not_found(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("FakeFs: {} not found", path.display()),
        )
    }

    fn dir_exists(files: &BTreeMap<PathBuf, FakeFile>, dir: &Path) -> bool {
        files.keys().any(|p| p != dir && p.starts_with(dir))
    }
}

/// A file of [`FakeFs::open_at`].
struct FakeReadAt(Vec<u8>);

impl ReadAt for FakeReadAt {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let bytes = usize::try_from(offset)
            .ok()
            .and_then(|start| self.0.get(start..)?.get(..buf.len()))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "FakeFs: read past the end")
            })?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

const DIR_META: FileMeta = FileMeta {
    len: 0,
    modified: None,
    is_dir: true,
};

impl Fs for FakeFs {
    fn read(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
        let files = lock(&self.files);
        let file = files.get(path).ok_or_else(|| FakeFs::not_found(path))?;
        if file.data.len() as u64 > max_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file larger than allowed",
            ));
        }
        Ok(file.data.clone())
    }

    fn open(&self, path: &Path, max_len: u64) -> io::Result<Box<dyn io::Read + Send>> {
        let data = self.read(path, max_len)?;
        Ok(Box::new(io::Cursor::new(data)))
    }

    fn read_tail(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
        let files = lock(&self.files);
        let file = files.get(path).ok_or_else(|| FakeFs::not_found(path))?;
        let keep = usize::try_from(max_len)
            .unwrap_or(usize::MAX)
            .min(file.data.len());
        Ok(file.data[file.data.len() - keep..].to_vec())
    }

    /// A snapshot of the file: later changes to the fake file system are not seen through it.
    fn open_at(&self, path: &Path) -> io::Result<Box<dyn ReadAt>> {
        let files = lock(&self.files);
        let file = files.get(path).ok_or_else(|| FakeFs::not_found(path))?;
        Ok(Box::new(FakeReadAt(file.data.clone())))
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMeta> {
        let files = lock(&self.files);
        if let Some(f) = files.get(path) {
            return Ok(FileMeta {
                len: f.data.len() as u64,
                modified: f.modified,
                is_dir: false,
            });
        }
        if FakeFs::dir_exists(&files, path) {
            return Ok(DIR_META);
        }
        Err(FakeFs::not_found(path))
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let files = lock(&self.files);
        if !FakeFs::dir_exists(&files, dir) {
            return Err(FakeFs::not_found(dir));
        }
        let mut out = Vec::new();
        let mut subdirs = BTreeSet::new();
        for (path, file) in files.iter() {
            let Ok(rest) = path.strip_prefix(dir) else {
                continue;
            };
            let mut parts = rest.components();
            let Some(first) = parts.next() else { continue };
            if parts.next().is_some() {
                subdirs.insert(dir.join(first));
            } else {
                let meta = FileMeta {
                    len: file.data.len() as u64,
                    modified: file.modified,
                    is_dir: false,
                };
                out.push(DirEntry {
                    path: path.clone(),
                    meta,
                });
            }
        }
        out.extend(subdirs.into_iter().map(|path| DirEntry {
            path,
            meta: DIR_META,
        }));
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

// ---- Secrets --------------------------------------------------------------------------------------

/// An in-memory secret store keyed by Credential Manager target name.
#[derive(Debug, Default)]
pub struct FakeSecrets {
    store: Mutex<BTreeMap<String, SecretString>>,
}

impl FakeSecrets {
    /// An empty store.
    pub fn new() -> FakeSecrets {
        FakeSecrets::default()
    }

    /// Stores an (invented) secret.
    pub fn with(self, name: &CredentialName, secret: &str) -> FakeSecrets {
        lock(&self.store).insert(name.target(), SecretString::new(secret.to_owned()));
        self
    }
}

impl Secrets for FakeSecrets {
    fn get(&self, name: &CredentialName) -> Result<Option<SecretString>, SecretsError> {
        Ok(lock(&self.store).get(&name.target()).cloned())
    }

    fn set(&self, name: &CredentialName, secret: &SecretString) -> Result<(), SecretsError> {
        lock(&self.store).insert(name.target(), secret.clone());
        Ok(())
    }

    fn delete(&self, name: &CredentialName) -> Result<(), SecretsError> {
        lock(&self.store).remove(&name.target());
        Ok(())
    }
}

// ---- Processes ------------------------------------------------------------------------------------

/// One scripted line of a fake child's stdout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeLine {
    /// A line (without line ending).
    Line(String),
    /// `read_line` times out once.
    Timeout,
    /// End of stream.
    Eof,
}

/// A spawner whose children print scripted lines. Each `spawn` takes the next script; with no
/// script left it fails with `NotFound` (like a missing executable).
#[derive(Debug, Default)]
pub struct FakeSpawner {
    scripts: Mutex<VecDeque<Vec<FakeLine>>>,
    spawned: Mutex<Vec<SpawnSpec>>,
    written: Arc<Mutex<Vec<String>>>,
    killed: Arc<Mutex<usize>>,
}

impl FakeSpawner {
    /// No scripts.
    pub fn new() -> FakeSpawner {
        FakeSpawner::default()
    }

    /// The next spawned child prints these lines, then reaches end of stream.
    pub fn script(&self, lines: Vec<FakeLine>) -> &FakeSpawner {
        lock(&self.scripts).push_back(lines);
        self
    }

    /// Every spawn request, in order.
    pub fn spawned(&self) -> Vec<SpawnSpec> {
        lock(&self.spawned).clone()
    }

    /// Every line written to any child's stdin, in order.
    pub fn written(&self) -> Vec<String> {
        lock(&self.written).clone()
    }

    /// How many children were killed (explicitly or by drop).
    pub fn killed(&self) -> usize {
        *lock(&self.killed)
    }
}

struct FakeChild {
    lines: VecDeque<FakeLine>,
    written: Arc<Mutex<Vec<String>>>,
    killed: Arc<Mutex<usize>>,
    alive: bool,
}

impl ChildIo for FakeChild {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        if !self.alive {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "child killed"));
        }
        lock(&self.written).push(line.to_owned());
        Ok(())
    }

    fn read_line(&mut self, _timeout: Duration) -> io::Result<Option<String>> {
        if !self.alive {
            return Ok(None);
        }
        match self.lines.pop_front() {
            Some(FakeLine::Line(l)) => Ok(Some(l)),
            Some(FakeLine::Timeout) => Err(io::Error::new(io::ErrorKind::TimedOut, "fake timeout")),
            Some(FakeLine::Eof) | None => Ok(None),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        if self.alive {
            self.alive = false;
            *lock(&self.killed) += 1;
        }
        Ok(())
    }
}

impl Drop for FakeChild {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

impl Spawner for FakeSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        lock(&self.spawned).push(spec.clone());
        let lines = lock(&self.scripts)
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "FakeSpawner: no script"))?;
        Ok(Box::new(FakeChild {
            lines: lines.into(),
            written: Arc::clone(&self.written),
            killed: Arc::clone(&self.killed),
            alive: true,
        }))
    }
}

// ---- Environment ----------------------------------------------------------------------------------

/// A fixed environment.
#[derive(Clone, Debug, Default)]
pub struct FakeEnv {
    vars: BTreeMap<String, OsString>,
    home: Option<PathBuf>,
}

impl FakeEnv {
    /// An environment with a home folder and no variables.
    pub fn new(home: impl Into<PathBuf>) -> FakeEnv {
        FakeEnv {
            vars: BTreeMap::new(),
            home: Some(home.into()),
        }
    }

    /// Sets a variable.
    pub fn with_var(mut self, name: &str, value: impl Into<OsString>) -> FakeEnv {
        self.vars.insert(name.to_owned(), value.into());
        self
    }
}

impl Env for FakeEnv {
    fn var(&self, name: &str) -> Option<OsString> {
        self.vars.get(name).cloned()
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

// ---- All together ---------------------------------------------------------------------------------

/// One of each fake, for building a [`FetchContext`] in a test.
#[derive(Debug)]
pub struct Fakes {
    /// Clock.
    pub clock: FakeClock,
    /// HTTP.
    pub http: FakeHttp,
    /// Files.
    pub fs: FakeFs,
    /// Secrets.
    pub secrets: FakeSecrets,
    /// Processes.
    pub spawner: FakeSpawner,
    /// Environment.
    pub env: FakeEnv,
}

impl Fakes {
    /// Fakes at `now` with home folder `C:\Users\tester` and nothing else.
    pub fn new(now: Timestamp) -> Fakes {
        Fakes {
            clock: FakeClock::new(now),
            http: FakeHttp::new(),
            fs: FakeFs::new(),
            secrets: FakeSecrets::new(),
            spawner: FakeSpawner::new(),
            env: FakeEnv::new(r"C:\Users\tester"),
        }
    }

    /// A context over these fakes with a deadline `budget` after the clock's now.
    pub fn context(&self, budget: SignedDuration) -> FetchContext<'_> {
        FetchContext {
            clock: &self.clock,
            http: &self.http,
            fs: &self.fs,
            secrets: &self.secrets,
            spawner: &self.spawner,
            env: &self.env,
            deadline: self.clock.now() + budget,
        }
    }
}
