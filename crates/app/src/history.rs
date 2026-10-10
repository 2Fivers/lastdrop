//! The history store (ARCHITECTURE "Persistence"): append-only JSONL, one file per meter in
//! `%LOCALAPPDATA%\Lastdrop\history\`, one [`Sample`] per line; thinned to one sample per meter
//! per 5 minutes; kept 90 days. It implements [`HistoryView`] for `lastdrop-core`'s statistics.
//!
//! In memory it keeps only what the statistics need (see [`HistoryView`]): the current window
//! instance of each window meter (≤ 31 days), 30 days of each balance, 14 local days for the
//! sparkline — the 10 MB budget (ARCHITECTURE goal 1) rules out more.
//!
//! Threading: main thread. Appends are small (one line per meter per reading).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{MeterKey, Snapshot};
use lastdrop_core::stats::{HistoryView, Sample};

/// How far back samples stay in memory: 31 days (a calendar-month window; balances need 30).
pub const IN_MEMORY_SPAN: SignedDuration = SignedDuration::from_hours(31 * 24);

/// The store.
pub struct HistoryStore {
    #[expect(dead_code, reason = "used once record/compact are implemented (M1)")]
    dir: PathBuf,
    samples: BTreeMap<MeterKey, Vec<Sample>>,
}

impl HistoryStore {
    /// Opens `dir` (created when missing) and loads the last [`IN_MEMORY_SPAN`] of every file.
    /// Unreadable lines are skipped (a crash can cut the last line).
    pub fn open(dir: &Path, now: Timestamp) -> std::io::Result<HistoryStore> {
        todo!("M1: open history")
    }

    /// The file of a meter: `<provider>.<account>.<meter>.jsonl`, with characters outside
    /// `[A-Za-z0-9_-]` replaced by `_` (`claude.default.weekly_fable.jsonl`).
    pub fn file_name(key: &MeterKey) -> String {
        todo!("M1: file name")
    }

    /// Records a snapshot: per meter, appends a sample when the last stored one is at least
    /// `HISTORY_THIN_INTERVAL` (5 min) older, or the window instance changed (R§3.1), or the
    /// meter became exhausted or not exhausted; otherwise the newest in-memory sample is replaced
    /// (the file keeps the 5-minute grid). Balances: one sample per amount change or 5 minutes.
    /// A sample's `t` is the snapshot's `measured_at` (`t_r`); a reading that is not newer than
    /// the meter's last sample (the same cached reading read again) is not recorded.
    pub fn record(&mut self, snapshot: &Snapshot) -> std::io::Result<()> {
        todo!("M1: record")
    }

    /// Drops in-memory samples the statistics no longer need (called on the minute tick, cheap):
    /// window meters keep their current window instance, the service's sparkline meter (F§6)
    /// also 14 local days; balances keep 30 days (R§3.6 `ref`); nothing older than
    /// [`IN_MEMORY_SPAN`]. At one sample per 5 minutes a meter kept for 31 days is ~9,000
    /// samples, so trimming to need is what keeps history inside the 10 MB budget.
    pub fn trim_memory(&mut self, now: Timestamp) {
        todo!("M1: trim memory")
    }

    /// Rewrites files without samples older than 90 days (`HISTORY_RETENTION`), atomically; once a
    /// day, off the critical path.
    pub fn compact(&mut self, now: Timestamp) -> std::io::Result<()> {
        todo!("M1: compact files")
    }
}

impl HistoryView for HistoryStore {
    fn samples(&self, key: &MeterKey, since: Timestamp) -> &[Sample] {
        match self.samples.get(key) {
            Some(v) => &v[v.partition_point(|s| s.t < since)..],
            None => &[],
        }
    }
}
