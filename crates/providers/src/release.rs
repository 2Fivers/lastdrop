//! Lastdrop's own releases (docs/design/updates-spec.md): the latest release of the public
//! releases repository through GitHub's REST API, and the download of a release file. The only
//! requests Lastdrop makes that are not to a service whose usage it reads; the user can turn
//! them off (updates-spec §1).
//!
//! The download streams to a file (the installer is ~17 MB, more than [`crate::io::MAX_BODY_BYTES`])
//! and follows GitHub's redirect to its storage itself: only to `https://` hosts of GitHub's
//! (`github.com`, `*.githubusercontent.com`), at most [`MAX_REDIRECTS`] times.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::io::{HeaderValue, Http, HttpError, HttpRequest, Method};

/// How long the "latest release" request may take.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a whole download may take.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Redirects a download follows (github.com → its storage).
pub const MAX_REDIRECTS: usize = 3;

/// What the check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Latest {
    /// The answer of `GET /repos/{repo}/releases/latest` (a release as JSON).
    Release(Vec<u8>),
    /// The repository has no published release yet (404).
    None,
}

/// Why a check or a download failed (no secrets: nothing secret is sent).
#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    /// No answer.
    #[error("{0}")]
    Http(#[from] HttpError),
    /// GitHub answered with this status (403/429: its unauthenticated rate limit).
    #[error("GitHub answered {0}")]
    Status(u16),
    /// A redirect led somewhere a download may not go.
    #[error("refused redirect to {0}")]
    Redirect(String),
    /// The file was larger than allowed.
    #[error("the file is larger than {0} bytes")]
    TooLarge(u64),
    /// Writing the file failed.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// The latest release of `repo` (`owner/name`).
pub fn latest(http: &dyn Http, repo: &str) -> Result<Latest, ReleaseError> {
    let response = http.request(&HttpRequest {
        method: Method::Get,
        url: format!("https://api.github.com/repos/{repo}/releases/latest"),
        headers: vec![
            (
                "Accept".to_owned(),
                HeaderValue::Plain("application/vnd.github+json".to_owned()),
            ),
            (
                "X-GitHub-Api-Version".to_owned(),
                HeaderValue::Plain("2022-11-28".to_owned()),
            ),
        ],
        body: None,
        timeout: CHECK_TIMEOUT,
    })?;
    match response.status {
        200 => Ok(Latest::Release(response.body)),
        404 => Ok(Latest::None),
        status => Err(ReleaseError::Status(status)),
    }
}

/// A host a download may be redirected to: GitHub's own, over `https://`.
pub fn allowed_download_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.contains(['@', ':']) {
        return false;
    }
    host == "github.com" || host.ends_with(".githubusercontent.com")
}

/// Downloads `url` (a GitHub release file) to `dest`, replacing it, at most `max` bytes;
/// returns the number of bytes written. A failed download leaves no file at `dest`.
pub fn download(url: &str, dest: &Path, max: u64) -> Result<u64, ReleaseError> {
    let result = download_inner(url, dest, max);
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn download_inner(url: &str, dest: &Path, max: u64) -> Result<u64, ReleaseError> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .max_idle_connections(0)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(ureq::Proxy::try_from_env())
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .user_agent(concat!("Lastdrop/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        if !allowed_download_url(&url) {
            return Err(ReleaseError::Redirect(url));
        }
        let mut response = agent
            .get(&url)
            .header("Accept", "application/octet-stream")
            .call()
            .map_err(|e| ReleaseError::Http(HttpError::Other(e.to_string())))?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            url = location;
            continue;
        }
        if status != 200 {
            return Err(ReleaseError::Status(status));
        }
        let mut reader = response.body_mut().with_config().limit(max).reader();
        let mut file = std::fs::File::create(dest)?;
        let mut buf = vec![0u8; 64 << 10];
        let mut written = 0u64;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(if written >= max {
                        ReleaseError::TooLarge(max)
                    } else {
                        ReleaseError::Io(e)
                    });
                }
            };
            file.write_all(&buf[..n])?;
            written += n as u64;
        }
        file.sync_all()?;
        return Ok(written);
    }
    Err(ReleaseError::Redirect(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::HttpResponse;

    struct Answer(u16, &'static str);

    impl Http for Answer {
        fn request(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            assert_eq!(
                request.url,
                "https://api.github.com/repos/2Fivers/lastdrop-releases/releases/latest"
            );
            Ok(HttpResponse::new(self.0, self.1))
        }
    }

    #[test]
    fn latest_reads_the_release_or_none() {
        let repo = "2Fivers/lastdrop-releases";
        assert_eq!(
            latest(&Answer(200, "{}"), repo).expect("ok"),
            Latest::Release(b"{}".to_vec())
        );
        assert_eq!(latest(&Answer(404, ""), repo).expect("ok"), Latest::None);
        assert!(matches!(
            latest(&Answer(403, ""), repo),
            Err(ReleaseError::Status(403))
        ));
    }

    #[test]
    fn downloads_go_to_github_only() {
        assert!(allowed_download_url(
            "https://github.com/2Fivers/lastdrop-releases/releases/download/v0.2.0/Lastdrop-Setup.exe"
        ));
        assert!(allowed_download_url(
            "https://objects.githubusercontent.com/github-production-release-asset/1?x=y"
        ));
        assert!(allowed_download_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1"
        ));
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://evil.example/github.com",
            "https://user@github.com/x",
            "https://github.com:444/x",
            "https://githubusercontent.com.evil/x",
            "",
        ] {
            assert!(!allowed_download_url(bad), "{bad}");
        }
    }
}
