//! Updates (docs/design/updates-spec.md): which release is newer than this build and which of
//! its files to take, from the answer of GitHub's "latest release" endpoint; the `SHA256SUMS`
//! file of a release. Pure: the requests are `lastdrop_providers::release`'s, the download and the
//! install the app's.

use std::cmp::Ordering;
use std::fmt;

use serde::Deserialize;

/// The public repository the releases are published to (`owner/name`): the source repository is
/// private, and GitHub serves a private repository's releases only with a token.
pub const RELEASES_REPO: &str = "2Fivers/lastdrop-releases";

/// The installer's file name in a release: the app itself, which installs itself when it runs
/// under this name (updates-spec §2).
pub const SETUP_ASSET: &str = "Lastdrop-Setup.exe";

/// The checksum file of a release: `sha256sum` format, one `<hex>  <name>` line per file.
pub const SUMS_ASSET: &str = "SHA256SUMS";

/// The largest installer accepted.
pub const MAX_SETUP_BYTES: u64 = 64 << 20;

/// A version `major.minor.patch`, optionally `-pre` (a pre-release sorts before the release).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    /// Major.
    pub major: u32,
    /// Minor.
    pub minor: u32,
    /// Patch.
    pub patch: u32,
    /// The pre-release part after `-`, if any (`rc.1`).
    pub pre: Option<String>,
}

impl Version {
    /// Parses `1.2.3`, `v1.2.3` or `1.2.3-rc.1`; build metadata (`+…`) is ignored.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s.split('+').next().unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = core.split('.');
        let mut next = || -> Option<u32> {
            let p = parts.next()?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let (major, minor, patch) = (next()?, next()?, next()?);
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// This build's version (`CARGO_PKG_VERSION` of the core crate, the workspace's).
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version {
            major: 0,
            minor: 0,
            patch: 0,
            pre: None,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

/// A file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// Its name.
    pub name: String,
    /// Where it downloads from (`https://github.com/…`, which redirects to GitHub's storage).
    pub url: String,
    /// Its size in bytes.
    pub size: u64,
}

/// A release newer than this build, with the files an update needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Its version.
    pub version: Version,
    /// Its page on GitHub (the release notes).
    pub page: String,
    /// The installer.
    pub setup: Asset,
    /// The checksums.
    pub sums: Asset,
}

#[derive(Deserialize)]
struct RawRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Why a release answer was not an offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoOffer {
    /// This build is as new or newer.
    UpToDate,
    /// The answer is not a release this build can use (draft, pre-release, no version, missing
    /// files, a file too large or not on `https://github.com/`).
    Unusable(String),
}

/// The offer in GitHub's "latest release" answer (`GET /repos/{repo}/releases/latest`) for a
/// build of version `current` (updates-spec §1).
pub fn offer(json: &[u8], current: &Version) -> Result<Offer, NoOffer> {
    let raw: RawRelease =
        serde_json::from_slice(json).map_err(|e| NoOffer::Unusable(e.to_string()))?;
    if raw.draft || raw.prerelease {
        return Err(NoOffer::Unusable("a draft or pre-release".to_owned()));
    }
    let version = Version::parse(&raw.tag_name)
        .ok_or_else(|| NoOffer::Unusable(format!("tag {:?} is not a version", raw.tag_name)))?;
    if version <= *current {
        return Err(NoOffer::UpToDate);
    }
    let asset = |name: &str| -> Result<Asset, NoOffer> {
        let a = raw
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| NoOffer::Unusable(format!("no {name}")))?;
        if !a.browser_download_url.starts_with("https://github.com/") {
            return Err(NoOffer::Unusable(format!("{name} is not on github.com")));
        }
        Ok(Asset {
            name: a.name.clone(),
            url: a.browser_download_url.clone(),
            size: a.size,
        })
    };
    let setup = asset(SETUP_ASSET)?;
    if setup.size == 0 || setup.size > MAX_SETUP_BYTES {
        return Err(NoOffer::Unusable(format!(
            "{SETUP_ASSET} is {} bytes",
            setup.size
        )));
    }
    Ok(Offer {
        version,
        page: raw.html_url,
        setup,
        sums: asset(SUMS_ASSET)?,
    })
}

/// The SHA-256 of `name` in a `SHA256SUMS` file (`<64 hex>  <name>` or `<hex> *<name>`), lower
/// case; `None` when the file does not list it.
pub fn checksum_of(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hex, rest) = line.trim().split_once(char::is_whitespace)?;
        let file = rest.trim_start().trim_start_matches('*');
        (file == name && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hex.to_ascii_lowercase())
    })
}

/// `bytes` as lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("version")
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(v("v0.1.0"), v("0.1.0"));
        assert!(v("0.1.1") > v("0.1.0"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
        for bad in ["", "1.2", "1.2.3.4", "a.b.c", "1.2.-3", "1.2.3-", "v"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    /// An invented answer in the shape of GitHub's REST API (fields Lastdrop does not read
    /// left out).
    fn answer(tag: &str, draft: bool, setup_url: &str, size: u64) -> String {
        format!(
            r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":false,
            "html_url":"https://github.com/2Fivers/lastdrop-releases/releases/tag/{tag}",
            "assets":[
              {{"name":"Lastdrop-Setup.exe","browser_download_url":"{setup_url}","size":{size}}},
              {{"name":"SHA256SUMS","browser_download_url":"https://github.com/2Fivers/lastdrop-releases/releases/download/{tag}/SHA256SUMS","size":90}}
            ]}}"#
        )
    }

    #[test]
    fn offers_only_newer_complete_releases() {
        let url = "https://github.com/2Fivers/lastdrop-releases/releases/download/v0.2.0/Lastdrop-Setup.exe";
        let current = v("0.1.0");
        let offer = offer(
            answer("v0.2.0", false, url, 17_000_000).as_bytes(),
            &current,
        )
        .expect("offer");
        assert_eq!(offer.version, v("0.2.0"));
        assert_eq!(offer.setup.url, url);
        assert_eq!(offer.sums.name, SUMS_ASSET);
        assert_eq!(
            super::offer(answer("v0.1.0", false, url, 1).as_bytes(), &current),
            Err(NoOffer::UpToDate)
        );
        for (json, why) in [
            (answer("v0.2.0", true, url, 1), "draft"),
            (answer("latest", false, url, 1), "tag"),
            (
                answer("v0.2.0", false, "https://evil.example/x.exe", 1),
                "host",
            ),
            (answer("v0.2.0", false, url, 0), "size"),
            (answer("v0.2.0", false, url, MAX_SETUP_BYTES + 1), "size"),
            ("{}".to_owned(), "empty"),
            ("[".to_owned(), "json"),
        ] {
            assert!(
                matches!(
                    super::offer(json.as_bytes(), &current),
                    Err(NoOffer::Unusable(_))
                ),
                "{why}"
            );
        }
    }

    #[test]
    fn checksums_are_found_by_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  Lastdrop-Setup.exe\n{b} *other.zip\nnot a line\n");
        assert_eq!(checksum_of(&sums, "Lastdrop-Setup.exe"), Some(a));
        assert_eq!(checksum_of(&sums, "other.zip"), Some("b".repeat(64)));
        assert_eq!(checksum_of(&sums, "missing"), None);
        assert_eq!(
            checksum_of("abc  Lastdrop-Setup.exe", "Lastdrop-Setup.exe"),
            None
        );
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
