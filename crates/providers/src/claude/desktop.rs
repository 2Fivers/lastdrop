//! Source 1: Claude Desktop's `plan-usage-history.json` (no token needed).
//!
//! The file lives in Claude Desktop's data folder (`AccountSource::Claude::desktop_dir`,
//! `%APPDATA%\Claude`). Shape, confirmed against a real install (key names only, version 2):
//!
//! ```text
//! {"version": 2, "samples": [{"t": <epoch ms>, "org": <organisation id>,
//!                             "u": {"fh": <percent>, "sd": <percent>, "xu": <percent>, …}}]}
//! ```
//!
//! `u` keys: `fh` five-hour, `sd` seven-day, `so` seven-day Opus, `sn` seven-day Sonnet (older
//! plans); `oa`, `cw`, `om`, `op` (OAuth apps, Cowork, Claude Design and its promotion) and `xu`
//! (extra usage) have no R§2 meter and are ignored. Samples are about 15 minutes apart, ascending,
//! kept 30 days. **Samples carry utilization only, no reset times**: the readings this source gives
//! have `resets_at: None` (the provider fills them in from another source's reading of the same
//! window instance when it has one). Desktop keeps the full usage response with `resets_at` only in
//! Chromium's HTTP cache (`Cache\Cache_Data`, block-file format, zstd-compressed bodies), which
//! Lastdrop does not read.
//!
//! The organisation id is skipped while parsing: never kept or logged.
//! Fixtures are written by hand from that shape with invented values
//! (tests/fixtures/claude/README.md).

use std::fmt;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use lastdrop_core::model::SourceKind;
use serde::Deserialize;
use serde::de::{Deserializer, SeqAccess, Visitor};

use super::json::{Instant, Lenient, WindowValue, Windows, json_detail};
use super::{ClaudeUsage, ClaudeWindow, WindowReading, file_error};
use crate::FetchContext;
use crate::error::FetchError;

/// File name of the history in the desktop data folder.
pub const FILE_NAME: &str = "plan-usage-history.json";

/// Largest file read (the history is small; anything bigger is not it).
pub const MAX_BYTES: u64 = 4 << 20;

const FROM: SourceKind = SourceKind::ClaudeDesktopHistory;

/// The history file of a desktop data folder.
pub fn path(desktop_dir: &Path) -> PathBuf {
    desktop_dir.join(FILE_NAME)
}

/// Reads and parses the newest entry of the history. `FetchError::Unavailable` when the file does
/// not exist (Claude Desktop not installed or never opened) or holds no usable sample yet.
pub fn read(ctx: &FetchContext<'_>, desktop_dir: &Path) -> Result<ClaudeUsage, FetchError> {
    let bytes = ctx
        .fs
        .read(&path(desktop_dir), MAX_BYTES)
        .map_err(|e| file_error(FROM, &e, FILE_NAME))?;
    parse(&bytes, ctx.clock.now())
}

/// Parses the file; tolerant of unknown and missing fields. `fetched_at` stands in for
/// `measured_at` when the entry carries no time.
///
/// The newest sample is the one with the latest `t` that has at least one usable window (a
/// sample without `t` loses to any sample with one; among equals the later in the file wins).
/// Samples that are not objects, values that are not numbers and unknown keys are skipped.
/// `FetchError::Unavailable` when no sample has a usable window; `FetchError::Parse` when the
/// file is not JSON of this shape.
pub fn parse(bytes: &[u8], fetched_at: Timestamp) -> Result<ClaudeUsage, FetchError> {
    let file: HistoryFile = serde_json::from_slice(bytes).map_err(|e| FetchError::Parse {
        from: FROM,
        detail: json_detail(FILE_NAME, &e),
    })?;
    let Some(newest) = file.samples.and_then(|s| s.0) else {
        return Err(FetchError::Unavailable {
            detail: format!("no usage samples in {FILE_NAME} yet"),
        });
    };
    Ok(ClaudeUsage {
        windows: newest.windows,
        plan: None,
        measured_at: Some(newest.t.unwrap_or(fetched_at)),
    })
}

#[derive(Deserialize)]
struct HistoryFile {
    /// `null` or missing: no samples. `version` and other keys are ignored.
    #[serde(default)]
    samples: Option<Newest>,
}

/// A history sample (`org` and unknown keys are skipped, never kept).
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawSample {
    t: Lenient<Instant>,
    u: Lenient<RawUsage>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawUsage {
    fh: Lenient<WindowValue>,
    sd: Lenient<WindowValue>,
    so: Lenient<WindowValue>,
    sn: Lenient<WindowValue>,
}

impl RawUsage {
    fn windows(&self) -> Vec<WindowReading> {
        let mut out = Windows::default();
        out.push(self.fh.0.and_then(|v| v.reading(ClaudeWindow::FiveHour)));
        out.push(self.sd.0.and_then(|v| v.reading(ClaudeWindow::Weekly)));
        out.push_model("Opus", self.so.0);
        out.push_model("Sonnet", self.sn.0);
        out.0
    }
}

/// The newest usable sample of the list, kept while streaming through it so the other samples
/// (hundreds of them) are never held at once.
struct Sample {
    t: Option<Timestamp>,
    windows: Vec<WindowReading>,
}

struct Newest(Option<Sample>);

impl<'de> Deserialize<'de> for Newest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Newest, D::Error> {
        deserializer.deserialize_seq(NewestVisitor)
    }
}

struct NewestVisitor;

impl<'de> Visitor<'de> for NewestVisitor {
    type Value = Newest;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of usage samples")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Newest, A::Error> {
        let mut best: Option<Sample> = None;
        while let Some(Lenient(raw)) = seq.next_element::<Lenient<RawSample>>()? {
            let Some(raw) = raw else { continue };
            let windows = raw.u.0.as_ref().map(RawUsage::windows).unwrap_or_default();
            if windows.is_empty() {
                continue;
            }
            let t = raw.t.0.map(|i| i.0);
            let newer = match (&best, t) {
                (None, _) | (Some(Sample { t: None, .. }), _) => true,
                (
                    Some(Sample {
                        t: Some(best_t), ..
                    }),
                    Some(t),
                ) => t >= *best_t,
                (Some(Sample { t: Some(_), .. }), None) => false,
            };
            if newer {
                best = Some(Sample { t, windows });
            }
        }
        Ok(Newest(best))
    }
}
