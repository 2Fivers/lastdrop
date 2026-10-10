//! Production implementations of the I/O traits: [`RealClock`], [`RealFs`], [`UreqHttp`] (TLS via
//! SChannel with the system roots, no async runtime), [`RealSpawner`] (`CREATE_NO_WINDOW`) and
//! [`RealEnv`]. The secret store's production implementation is Credential Manager in
//! `lastdrop-win::credentials`, adapted to [`crate::io::Secrets`] by the app.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;

use crate::io::{
    ChildIo, Clock, DirEntry, Env, FileMeta, Fs, Http, HttpError, HttpRequest, HttpResponse,
    SpawnSpec, Spawner, wildcard_match,
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

/// The blocking HTTP client: `ureq` with native-tls (SChannel) and the Windows root store, no
/// redirects (a redirect off the documented endpoint is an error), 4xx/5xx returned as
/// responses, bodies capped at [`crate::io::MAX_BODY_BYTES`].
pub struct UreqHttp {
    #[expect(dead_code, reason = "read by request() once implemented (M1)")]
    agent: ureq::Agent,
}

impl UreqHttp {
    /// `User-Agent: Lastdrop/<version>`.
    pub fn new() -> UreqHttp {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .max_redirects(0)
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

impl Http for UreqHttp {
    #[expect(unused_variables, reason = "M1 contract stub")]
    fn request(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        todo!("M1: send with ureq, per-request timeout, map transport errors to HttpError")
    }
}

/// Starts processes with `CREATE_NO_WINDOW` (0x0800_0000) through
/// `std::os::windows::process::CommandExt`, piped stdin/stdout, stderr to null. A reader thread
/// per child turns stdout into lines so [`ChildIo::read_line`] can time out.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealSpawner;

/// `CREATE_NO_WINDOW` process creation flag.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

impl Spawner for RealSpawner {
    #[expect(unused_variables, reason = "M1 contract stub")]
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        todo!("M1: std::process::Command + CREATE_NO_WINDOW + line reader thread")
    }
}

/// Default timeout of one HTTP call when the provider does not say (**not from the spec**).
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);
