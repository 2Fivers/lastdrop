//! The history store (ARCHITECTURE "Persistence"): append-only JSONL, one file per meter in
//! `%LOCALAPPDATA%\Lastdrop\history\`, one [`Sample`] per line; thinned to one sample per meter
//! per 5 minutes; kept 90 days. It implements [`HistoryView`] for `lastdrop-core`'s statistics.
//!
//! In memory it keeps only what the statistics need (see [`HistoryView`]): the current window
//! instance of each window meter (≤ 31 days), 30 days of each balance, 14 local days for the
//! sparkline — the 10 MB budget (ARCHITECTURE goal 1) rules out more.
//!
//! Crash safety: a sample is appended as one complete line in one write to a file opened for
//! appending, so a crash can at worst cut the last line. Loading skips lines that do not parse,
//! and the first append after a cut line starts on a new line, so the cut never spreads to the
//! next sample. Files are only ever rewritten by [`HistoryStore::compact`], through a temporary
//! file that replaces the original in one rename.
//!
//! In memory, each meter holds the samples that are on disk plus at most one newer reading that
//! is not (the latest reading inside the current 5-minute step), so statistics always see the
//! latest reading.
//!
//! Threading: main thread. Appends are small (one line per meter per reading).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::consts::{
    BAL_REF_LOOKBACK, HISTORY_RETENTION, HISTORY_THIN_INTERVAL, SPARK_DAYS,
};
use lastdrop_core::model::{
    MeterId, MeterKey, MeterKind, ProviderId, Snapshot, WindowLen, catalog,
};
use lastdrop_core::stats::{
    HistoryView, Sample, current_instance, instance_of, is_exhausted, latest_instance,
    same_window_instance,
};

/// How far back samples stay in memory: 31 days (a calendar-month window; balances need 30).
pub const IN_MEMORY_SPAN: SignedDuration = SignedDuration::from_hours(31 * 24);

/// How much of a service's sparkline meter stays in memory (F§6): its 14 local days, the day
/// before the first of them (`flyout::spark_values` reads from 24 h before it), and one more day
/// for time zones and daylight saving.
const SPARK_SPAN: SignedDuration = SignedDuration::from_hours((SPARK_DAYS as i64 + 2) * 24);

/// Samples this far ahead of the clock are not loaded: written while the clock was wrong, they
/// would make every newer reading look old (**not from the spec**).
const FUTURE_TOLERANCE: SignedDuration = SignedDuration::from_hours(1);

/// Longest line read from a history file; a sample line is under 100 bytes.
const MAX_LINE: u64 = 4096;

/// File name suffix of a meter's history.
const EXTENSION: &str = ".jsonl";

/// Suffix of [`HistoryStore::compact`]'s temporary files (`<meter file>.tmp`).
const TEMP_SUFFIX: &str = ".jsonl.tmp";

/// The store.
pub struct HistoryStore {
    dir: PathBuf,
    /// Keyed by file name ([`HistoryStore::file_name`]): the file is a meter's identity on disk,
    /// and a file can be loaded before the meter's [`MeterKey`] is seen again.
    series: BTreeMap<String, Series>,
}

/// What kind of meter a series holds, which decides how much of it stays in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A window meter with its length.
    Window(WindowLen),
    /// A balance meter.
    Balance,
    /// Loaded from a file and not read since: keep up to [`IN_MEMORY_SPAN`].
    Unknown,
}

/// One meter's history in memory.
#[derive(Debug)]
struct Series {
    /// `<provider>.<account>`, sanitized: the service the meter belongs to.
    service: String,
    /// The meter part of the file name, sanitized.
    meter: String,
    kind: Kind,
    /// Ascending by `t`.
    samples: Vec<Sample>,
    /// `t` of the newest sample in the file.
    last_persisted: Option<Timestamp>,
    /// The last of `samples` is not in the file (it replaces itself until the next step).
    tail_pending: bool,
    /// Window meters: whether the latest reading was exhausted (R§3.3).
    exhausted: Option<bool>,
    /// The file ends with a cut line: the next append starts with a line break.
    needs_newline: bool,
}

impl Series {
    fn new(service: String, meter: String, kind: Kind) -> Series {
        Series {
            service,
            meter,
            kind,
            samples: Vec::new(),
            last_persisted: None,
            tail_pending: false,
            exhausted: None,
            needs_newline: false,
        }
    }
}

/// `t − d`, saturating at the earliest timestamp.
fn sub(t: Timestamp, d: SignedDuration) -> Timestamp {
    t.checked_sub(d).unwrap_or(Timestamp::MIN)
}

/// `t + d`, saturating at the latest timestamp.
fn add(t: Timestamp, d: SignedDuration) -> Timestamp {
    t.checked_add(d).unwrap_or(Timestamp::MAX)
}

/// `part` with characters outside `[A-Za-z0-9_-]` replaced by `_`.
fn sanitize(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `(service, meter)` of a history file name, `None` for any other file.
fn split_file_name(name: &str) -> Option<(String, String)> {
    let stem = name.strip_suffix(EXTENSION)?;
    let mut parts = stem.split('.');
    let (provider, account, meter) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || provider.is_empty() || meter.is_empty() {
        return None;
    }
    Some((format!("{provider}.{account}"), meter.to_owned()))
}

/// The kind of a meter known from the R§2 catalogue, before any reading of it.
fn catalogue_kind(service: &str, meter: &str) -> Kind {
    let provider = service.split('.').next().and_then(ProviderId::from_id);
    let def = provider.and_then(|p| catalog(p).iter().find(|d| sanitize(d.id) == meter));
    match def.map(|d| d.kind) {
        Some(MeterKind::Window { window, .. }) => Kind::Window(window),
        Some(MeterKind::Balance) => Kind::Balance,
        None => Kind::Unknown,
    }
}

/// Reads the next line (at most [`MAX_LINE`] bytes) into `buf`; `None` at end of file; an empty
/// `buf` for a longer line, whose rest is skipped.
fn next_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> io::Result<Option<()>> {
    buf.clear();
    let n = reader.by_ref().take(MAX_LINE).read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && n as u64 == MAX_LINE {
        reader.skip_until(b'\n')?;
        buf.clear();
    }
    Ok(Some(()))
}

/// A line as a sample, `None` when it does not parse (a cut or corrupted line).
fn parse_line(line: &[u8]) -> Option<Sample> {
    let sample: Sample = serde_json::from_slice(line.trim_ascii()).ok()?;
    sample.value.is_finite().then_some(sample)
}

/// What [`load_file`] read.
struct Loaded {
    samples: Vec<Sample>,
    last_persisted: Option<Timestamp>,
    needs_newline: bool,
}

/// The samples of one file with `since ≤ t ≤ until`, ascending (one per `t`, the later line
/// wins); unreadable lines skipped.
fn load_file(path: &Path, since: Timestamp, until: Timestamp) -> io::Result<Loaded> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buf = Vec::new();
    let mut samples = Vec::new();
    let mut last_persisted: Option<Timestamp> = None;
    while next_line(&mut reader, &mut buf)?.is_some() {
        let Some(sample) = parse_line(&buf) else {
            continue;
        };
        if sample.t > until {
            continue;
        }
        last_persisted = last_persisted.max(Some(sample.t));
        if sample.t >= since {
            samples.push(sample);
        }
    }
    // Lines are written in order; a clock that jumped can still leave them out of order.
    samples.sort_by_key(|s| s.t);
    samples.dedup_by(|later, earlier| {
        let same = later.t == earlier.t;
        if same {
            *earlier = *later;
        }
        same
    });
    let mut file = reader.into_inner();
    let needs_newline = if file.metadata()?.len() == 0 {
        false
    } else {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        last[0] != b'\n'
    };
    Ok(Loaded {
        samples,
        last_persisted,
        needs_newline,
    })
}

/// Appends `samples` to `path` as complete lines in one write.
fn append(path: &Path, needs_newline: bool, samples: &[Sample]) -> io::Result<()> {
    let mut text = String::new();
    if needs_newline {
        text.push('\n');
    }
    for sample in samples {
        text.push_str(&serde_json::to_string(sample).map_err(io::Error::other)?);
        text.push('\n');
    }
    let open = || OpenOptions::new().create(true).append(true).open(path);
    let mut file = match open() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // The folder was removed while we ran: make it again.
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            open()?
        }
        other => other?,
    };
    file.write_all(text.as_bytes())
}

/// What [`compact_file`] did.
#[derive(Debug, PartialEq, Eq)]
enum Compacted {
    /// Nothing older than the retention: left as it was.
    Untouched,
    /// Rewritten without old or unreadable lines.
    Rewritten,
    /// Nothing left to keep: deleted.
    Removed,
}

/// Rewrites `path` without samples older than `cutoff` (and without unreadable lines), through
/// `<path>.tmp` and one rename; deletes it when nothing is left.
fn compact_file(path: &Path, cutoff: Timestamp) -> io::Result<Compacted> {
    let mut buf = Vec::new();
    {
        // The oldest sample is on the first line: when it is readable and recent, so is the file.
        let mut reader = BufReader::new(File::open(path)?);
        if next_line(&mut reader, &mut buf)?.is_some()
            && parse_line(&buf).is_some_and(|s| s.t >= cutoff)
        {
            return Ok(Compacted::Untouched);
        }
    }
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let result = rewrite(path, &tmp, cutoff, &mut buf);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// [`compact_file`]'s rewrite: the samples of `path` since `cutoff` into `tmp`, flushed to disk,
/// then `tmp` replaces `path` in one rename (or both go when nothing is kept).
fn rewrite(path: &Path, tmp: &Path, cutoff: Timestamp, buf: &mut Vec<u8>) -> io::Result<Compacted> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut writer = BufWriter::new(File::create(tmp)?);
    let mut kept = 0usize;
    while next_line(&mut reader, buf)?.is_some() {
        if let Some(sample) = parse_line(buf).filter(|s| s.t >= cutoff) {
            serde_json::to_writer(&mut writer, &sample).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            kept += 1;
        }
    }
    drop(reader);
    let file = writer.into_inner()?;
    file.sync_all()?;
    drop(file);
    if kept == 0 {
        fs::remove_file(path)?;
        fs::remove_file(tmp)?;
        return Ok(Compacted::Removed);
    }
    fs::rename(tmp, path)?;
    Ok(Compacted::Rewritten)
}

/// The window length used to pick a service's sparkline meter (`flyout::spark_meter`: `weekly`,
/// else the longest window).
fn window_rank(kind: Kind) -> SignedDuration {
    match kind {
        Kind::Window(WindowLen::Fixed(len)) => len,
        Kind::Window(WindowLen::CalendarMonth) => SignedDuration::from_hours(31 * 24),
        Kind::Window(WindowLen::Unknown) | Kind::Balance | Kind::Unknown => SignedDuration::ZERO,
    }
}

impl HistoryStore {
    /// Opens `dir` (created when missing) and loads the last [`IN_MEMORY_SPAN`] of every file.
    /// Unreadable lines are skipped (a crash can cut the last line).
    pub fn open(dir: &Path, now: Timestamp) -> io::Result<HistoryStore> {
        fs::create_dir_all(dir)?;
        let mut store = HistoryStore {
            dir: dir.to_owned(),
            series: BTreeMap::new(),
        };
        let since = sub(now, IN_MEMORY_SPAN);
        let until = add(now, FUTURE_TOLERANCE);
        for entry in fs::read_dir(dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    log::warn!("history: cannot list a file: {e}");
                    continue;
                }
            };
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.ends_with(TEMP_SUFFIX) {
                // Left by a compaction that did not finish; the original is intact.
                if let Err(e) = fs::remove_file(entry.path()) {
                    log::warn!("history: cannot remove {name}: {e}");
                }
                continue;
            }
            let Some((service, meter)) = split_file_name(name) else {
                continue;
            };
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            match load_file(&entry.path(), since, until) {
                Ok(loaded) => {
                    let kind = catalogue_kind(&service, &meter);
                    let mut series = Series::new(service, meter, kind);
                    series.exhausted = match (kind, loaded.samples.last()) {
                        (Kind::Window(_), Some(last)) => Some(last.value >= 1.0),
                        _ => None,
                    };
                    series.samples = loaded.samples;
                    series.last_persisted = loaded.last_persisted;
                    series.needs_newline = loaded.needs_newline;
                    store.series.insert(name.to_owned(), series);
                }
                Err(e) => log::warn!("history: cannot read {name}: {e}"),
            }
        }
        store.trim_memory(now);
        Ok(store)
    }

    /// The file of a meter: `<provider>.<account>.<meter>.jsonl`, with characters outside
    /// `[A-Za-z0-9_-]` replaced by `_` (`claude.default.weekly_fable.jsonl`).
    pub fn file_name(key: &MeterKey) -> String {
        format!(
            "{}.{}.{}{EXTENSION}",
            sanitize(key.provider.as_str()),
            sanitize(&key.account.0),
            sanitize(&key.meter.0)
        )
    }

    /// Records a snapshot: per meter, appends a sample when the last stored one is at least
    /// `HISTORY_THIN_INTERVAL` (5 min) older, or the window instance changed (R§3.1), or the
    /// meter became exhausted or not exhausted; otherwise the newest in-memory sample is replaced
    /// (the file keeps the 5-minute grid). Balances: one sample per amount change or 5 minutes.
    /// A sample's `t` is the snapshot's `measured_at` (`t_r`); a reading that is not newer than
    /// the meter's last sample (the same cached reading read again) is not recorded.
    ///
    /// A failed append keeps the reading in memory (it is written with the next one) and is
    /// returned after the other meters are recorded.
    pub fn record(&mut self, snapshot: &Snapshot) -> io::Result<()> {
        let t = snapshot.reading_at();
        let key = |meter: &MeterId| MeterKey {
            provider: snapshot.provider,
            account: snapshot.account.key.clone(),
            meter: meter.clone(),
        };
        let mut first_error = None;
        for meter in &snapshot.meters {
            let sample = Sample::window(t, meter.used, meter.resets_at);
            let exhausted = Some(is_exhausted(meter.used, meter.limited));
            let kind = Kind::Window(meter.window);
            if let Err(e) = self.add(&key(&meter.id), kind, sample, exhausted) {
                first_error.get_or_insert(e);
            }
        }
        for balance in &snapshot.balances {
            let sample = Sample::balance(t, balance.amount);
            if let Err(e) = self.add(&key(&balance.id), Kind::Balance, sample, None) {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// One reading of one meter (see [`HistoryStore::record`]).
    fn add(
        &mut self,
        key: &MeterKey,
        kind: Kind,
        sample: Sample,
        exhausted: Option<bool>,
    ) -> io::Result<()> {
        if !sample.value.is_finite() {
            return Ok(());
        }
        let name = HistoryStore::file_name(key);
        let series = self.series.entry(name.clone()).or_insert_with(|| {
            let service = format!(
                "{}.{}",
                sanitize(key.provider.as_str()),
                sanitize(&key.account.0)
            );
            Series::new(service, sanitize(&key.meter.0), kind)
        });
        series.kind = kind;
        let newest = series
            .samples
            .last()
            .map(|s| s.t)
            .max(series.last_persisted);
        if newest.is_some_and(|n| sample.t <= n) {
            return Ok(());
        }
        let last = series.samples.last().copied();
        let due = series
            .last_persisted
            .is_none_or(|p| sample.t.duration_since(p) >= HISTORY_THIN_INTERVAL);
        // A reading without a reset time inside the current instance belongs to it
        // (`instance_of`): a source without reset times does not start a new instance.
        let new_instance = kind != Kind::Balance && last.is_some() && {
            let last_instance = latest_instance(&series.samples);
            !same_window_instance(last_instance, instance_of(last_instance, &sample))
        };
        let changed = match last {
            None => true,
            Some(_) if kind == Kind::Balance => last.is_some_and(|l| l.value != sample.value),
            Some(_) => new_instance || series.exhausted != exhausted,
        };
        let mut result = Ok(());
        if due || changed {
            // The last reading of a window instance is kept too: the sparkline counts the
            // increases within each instance (F§6).
            let pending = last.filter(|_| series.tail_pending && new_instance);
            let lines: Vec<Sample> = pending.into_iter().chain([sample]).collect();
            result = append(&self.dir.join(&name), series.needs_newline, &lines);
            match &result {
                Ok(()) => {
                    series.needs_newline = false;
                    series.last_persisted = Some(sample.t);
                }
                Err(e) => log::warn!("history: cannot append to {name}: {e}"),
            }
        }
        let persisted = (due || changed) && result.is_ok();
        let keep_pending = series.tail_pending && !(persisted && new_instance);
        match series.samples.last_mut() {
            Some(tail) if keep_pending => *tail = sample,
            _ => series.samples.push(sample),
        }
        series.tail_pending = !persisted;
        series.exhausted = exhausted;
        result
    }

    /// Drops in-memory samples the statistics no longer need (called on the minute tick, cheap):
    /// window meters keep their current window instance, the service's sparkline meter (F§6)
    /// also 14 local days; balances keep 30 days (R§3.6 `ref`); nothing older than
    /// [`IN_MEMORY_SPAN`]. At one sample per 5 minutes a meter kept for 31 days is ~9,000
    /// samples, so trimming to need is what keeps history inside the 10 MB budget.
    pub fn trim_memory(&mut self, now: Timestamp) {
        let floor = sub(now, IN_MEMORY_SPAN);
        let spark_since = sub(now, SPARK_SPAN);
        let spark = self.sparkline_series();
        for (name, series) in &mut self.series {
            // A stale meter is judged at its reading (R§3.8), so its needs count from there.
            let anchor = series.samples.last().map_or(now, |s| s.t.min(now));
            let need = match series.kind {
                Kind::Balance => sub(anchor, BAL_REF_LOOKBACK),
                Kind::Window(_) => {
                    let start = current_instance(&series.samples)
                        .first()
                        .map_or(anchor, |s| s.t);
                    if spark.contains(name) {
                        start.min(spark_since)
                    } else {
                        start
                    }
                }
                Kind::Unknown => floor,
            };
            let keep_since = need.max(floor);
            let cut = series.samples.partition_point(|s| s.t < keep_since);
            if cut > 0 {
                series.samples.drain(..cut);
                if series.samples.capacity() > 2 * series.samples.len() + 64 {
                    series.samples.shrink_to_fit();
                }
            }
            if series.samples.is_empty() {
                series.tail_pending = false;
            }
        }
    }

    /// The file names of each service's sparkline meter (`flyout::spark_meter`): its `weekly`
    /// window, else its longest window (all of them on a tie).
    fn sparkline_series(&self) -> BTreeSet<String> {
        let mut by_service: BTreeMap<&str, Vec<(&str, &Series)>> = BTreeMap::new();
        for (name, series) in &self.series {
            if matches!(series.kind, Kind::Window(_)) {
                by_service
                    .entry(series.service.as_str())
                    .or_default()
                    .push((name.as_str(), series));
            }
        }
        let mut chosen = BTreeSet::new();
        for windows in by_service.values() {
            if let Some((name, _)) = windows.iter().find(|(_, s)| s.meter == MeterId::WEEKLY) {
                chosen.insert((*name).to_owned());
                continue;
            }
            let longest = windows
                .iter()
                .map(|(_, s)| window_rank(s.kind))
                .max()
                .unwrap_or_default();
            chosen.extend(
                windows
                    .iter()
                    .filter(|(_, s)| window_rank(s.kind) == longest)
                    .map(|(name, _)| (*name).to_owned()),
            );
        }
        chosen
    }

    /// Rewrites files without samples older than 90 days (`HISTORY_RETENTION`), atomically; once a
    /// day, off the critical path.
    ///
    /// A file with nothing left is deleted. A file whose oldest sample is recent is not touched.
    /// Errors on one file do not stop the others; the first is returned.
    pub fn compact(&mut self, now: Timestamp) -> io::Result<()> {
        let cutoff = sub(now, HISTORY_RETENTION);
        let mut first_error = None;
        for entry in fs::read_dir(&self.dir)? {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(e) => {
                    first_error.get_or_insert(e);
                    continue;
                }
            };
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if split_file_name(name).is_none() || !path.is_file() {
                continue;
            }
            let name = name.to_owned();
            match compact_file(&path, cutoff) {
                Ok(Compacted::Untouched) => {}
                Ok(Compacted::Rewritten) => {
                    if let Some(series) = self.series.get_mut(&name) {
                        series.needs_newline = false;
                    }
                }
                Ok(Compacted::Removed) => {
                    let empty = self.series.get_mut(&name).is_some_and(|series| {
                        series.needs_newline = false;
                        series.last_persisted = None;
                        series.samples.is_empty()
                    });
                    if empty {
                        self.series.remove(&name);
                    }
                    log::info!("history: removed {name} (nothing newer than 90 days)");
                }
                Err(e) => {
                    log::warn!("history: cannot compact {name}: {e}");
                    first_error.get_or_insert(e);
                }
            }
        }
        // In memory nothing is older than IN_MEMORY_SPAN, which is shorter than the retention.
        first_error.map_or(Ok(()), Err)
    }
}

impl HistoryView for HistoryStore {
    fn samples(&self, key: &MeterKey, since: Timestamp) -> &[Sample] {
        match self.series.get(&HistoryStore::file_name(key)) {
            Some(series) => {
                let v = &series.samples;
                &v[v.partition_point(|s| s.t < since)..]
            }
            None => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use jiff::{SignedDuration, Timestamp};
    use lastdrop_core::model::{
        AccountKey, AccountRef, Balance, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId,
        Snapshot, SourceKind, Unit, WindowLen,
    };
    use lastdrop_core::stats::{HistoryView, Sample};

    use super::{HistoryStore, IN_MEMORY_SPAN};

    /// A folder of the test's own under %TEMP%, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "lastdrop-history-test-{}-{n}-{tag}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            TempDir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn t0() -> Timestamp {
        "2026-10-10T10:42:00Z".parse().expect("timestamp")
    }

    fn at(minutes: i64) -> Timestamp {
        t0() + SignedDuration::from_mins(minutes)
    }

    fn days(n: i64) -> SignedDuration {
        SignedDuration::from_hours(24 * n)
    }

    fn key(provider: ProviderId, meter: &str) -> MeterKey {
        MeterKey {
            provider,
            account: AccountKey::primary(),
            meter: MeterId::new(meter),
        }
    }

    fn meter(id: &str, used: f64, resets_at: Option<Timestamp>, window: WindowLen) -> Meter {
        Meter {
            id: MeterId::new(id),
            names: MeterNames::new("x", "x", "x"),
            order: 0,
            used,
            limited: false,
            resets_at,
            window,
            starts_on_use: false,
        }
    }

    fn five_hour(used: f64, resets_at: Timestamp) -> Meter {
        meter(
            MeterId::FIVE_HOUR,
            used,
            Some(resets_at),
            WindowLen::Fixed(SignedDuration::from_hours(5)),
        )
    }

    fn snapshot(t: Timestamp, meters: Vec<Meter>, balances: Vec<Balance>) -> Snapshot {
        let provider = if balances.is_empty() {
            ProviderId::Claude
        } else {
            ProviderId::Meshy
        };
        Snapshot {
            provider,
            account: AccountRef::primary(),
            source: SourceKind::ClaudeOAuthUsage,
            measured_at: t,
            fetched_at: t,
            meters,
            balances,
            health: Health::Ok,
        }
    }

    fn credits(amount: f64) -> Balance {
        Balance {
            id: MeterId::new(MeterId::CREDITS),
            names: MeterNames::new("Cr", "credits", "Credits"),
            order: 0,
            amount,
            unit: Unit::Credits,
            last_top_up: None,
        }
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .expect("history file")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn write_samples(path: &Path, samples: &[Sample]) {
        let text: String = samples
            .iter()
            .map(|s| serde_json::to_string(s).expect("json") + "\n")
            .collect();
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, text).expect("write");
    }

    fn times(samples: &[Sample]) -> Vec<Timestamp> {
        samples.iter().map(|s| s.t).collect()
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(
            HistoryStore::file_name(&key(ProviderId::Claude, "weekly:fable")),
            "claude.default.weekly_fable.jsonl"
        );
        let odd = MeterKey {
            provider: ProviderId::Codex,
            account: AccountKey("Work acc/1".to_owned()),
            meter: MeterId::new("a.b-c_d"),
        };
        assert_eq!(
            HistoryStore::file_name(&odd),
            "codex.Work_acc_1.a_b-c_d.jsonl"
        );
    }

    #[test]
    fn thinning_keeps_a_five_minute_grid_on_disk_and_the_latest_reading_in_memory() {
        let dir = TempDir::new("thin");
        let mut store = HistoryStore::open(&dir.0, t0()).expect("open");
        let reset = at(240);
        for m in 0..12 {
            let used = 0.10 + 0.01 * m as f64;
            store
                .record(&snapshot(at(m), vec![five_hour(used, reset)], Vec::new()))
                .expect("record");
        }
        let k = key(ProviderId::Claude, MeterId::FIVE_HOUR);
        let in_memory = store.samples(&k, Timestamp::MIN);
        assert_eq!(times(in_memory), vec![at(0), at(5), at(10), at(11)]);
        assert!((in_memory[3].value - 0.21).abs() < 1e-12, "latest reading");
        let file = dir.file("claude.default.five_hour.jsonl");
        assert_eq!(lines(&file).len(), 3);

        let reopened = HistoryStore::open(&dir.0, at(12)).expect("reopen");
        assert_eq!(
            times(reopened.samples(&k, Timestamp::MIN)),
            vec![at(0), at(5), at(10)]
        );
        assert_eq!(reopened.samples(&k, at(5)).len(), 2);
        assert!(
            reopened
                .samples(&key(ProviderId::Codex, "weekly"), Timestamp::MIN)
                .is_empty()
        );
    }

    #[test]
    fn a_new_window_instance_and_exhaustion_are_stored_at_once() {
        let dir = TempDir::new("instance");
        let mut store = HistoryStore::open(&dir.0, t0()).expect("open");
        let k = key(ProviderId::Claude, MeterId::FIVE_HOUR);
        // With a weekly meter beside it the 5-hour meter is not the sparkline's (F§6), so memory
        // keeps only its current window instance.
        let weekly = meter(
            MeterId::WEEKLY,
            0.2,
            Some(at(5000)),
            WindowLen::Fixed(SignedDuration::from_hours(168)),
        );
        let record = |store: &mut HistoryStore, m: i64, five: Meter| {
            store
                .record(&snapshot(at(m), vec![five, weekly.clone()], Vec::new()))
                .expect("record");
        };
        let five = |used: f64, reset: i64| five_hour(used, at(reset));
        record(&mut store, 0, five(0.90, 10));
        record(&mut store, 1, five(0.95, 10)); // pending
        record(&mut store, 2, five(0.0, 310)); // new instance: the pending reading is kept too
        record(&mut store, 3, five(1.0, 310)); // exhausted
        record(&mut store, 4, five(1.0, 310)); // pending
        let mut limited = five(0.99, 310);
        limited.limited = true;
        // Still exhausted (limited), so only the 5-minute step stores it.
        record(&mut store, 7, limited.clone());
        record(&mut store, 8, limited);
        let file = dir.file("claude.default.five_hour.jsonl");
        assert_eq!(lines(&file).len(), 5, "0, 1, 2, 3 and 8");
        assert_eq!(
            times(store.samples(&k, Timestamp::MIN)),
            vec![at(0), at(1), at(2), at(3), at(8)]
        );
        store.trim_memory(at(9));
        assert_eq!(
            times(store.samples(&k, Timestamp::MIN)),
            vec![at(2), at(3), at(8)],
            "the current instance"
        );
        let reopened = HistoryStore::open(&dir.0, at(9)).expect("reopen");
        let on_disk = times(reopened.samples(&k, Timestamp::MIN));
        assert_eq!(on_disk, vec![at(2), at(3), at(8)]);
    }

    #[test]
    fn a_reading_without_a_reset_time_stays_in_the_instance() {
        let dir = TempDir::new("noreset");
        let mut store = HistoryStore::open(&dir.0, t0()).expect("open");
        let k = key(ProviderId::Claude, MeterId::FIVE_HOUR);
        let weekly = meter(
            MeterId::WEEKLY,
            0.2,
            Some(at(5000)),
            WindowLen::Fixed(SignedDuration::from_hours(168)),
        );
        let without_reset = |used: f64| {
            meter(
                MeterId::FIVE_HOUR,
                used,
                None,
                WindowLen::Fixed(SignedDuration::from_hours(5)),
            )
        };
        let record = |store: &mut HistoryStore, m: i64, five: Meter| {
            store
                .record(&snapshot(at(m), vec![five, weekly.clone()], Vec::new()))
                .expect("record");
        };
        record(&mut store, 0, five_hour(0.50, at(100)));
        record(&mut store, 5, five_hour(0.55, at(100)));
        // A desktop-history reading without `R`: the same instance, so only the 5-minute grid
        // stores it (minute 6 replaces itself), and trimming keeps the instance.
        record(&mut store, 6, without_reset(0.56));
        record(&mut store, 7, without_reset(0.57));
        assert_eq!(lines(&dir.file("claude.default.five_hour.jsonl")).len(), 2);
        store.trim_memory(at(8));
        assert_eq!(
            times(store.samples(&k, Timestamp::MIN)),
            vec![at(0), at(5), at(7)]
        );
        // After the reset time a reading without `R` belongs to no known instance.
        record(&mut store, 101, without_reset(0.0));
        assert_eq!(lines(&dir.file("claude.default.five_hour.jsonl")).len(), 4);
    }

    #[test]
    fn the_same_reading_again_is_not_recorded() {
        let dir = TempDir::new("same");
        let mut store = HistoryStore::open(&dir.0, t0()).expect("open");
        let k = key(ProviderId::Claude, MeterId::FIVE_HOUR);
        let snap = snapshot(at(0), vec![five_hour(0.4, at(100))], Vec::new());
        store.record(&snap).expect("record");
        store.record(&snap).expect("again");
        store
            .record(&snapshot(at(-3), vec![five_hour(0.3, at(100))], Vec::new()))
            .expect("older");
        assert_eq!(times(store.samples(&k, Timestamp::MIN)), vec![at(0)]);
        assert_eq!(lines(&dir.file("claude.default.five_hour.jsonl")).len(), 1);
        // Not a number: never stored.
        store
            .record(&snapshot(
                at(9),
                vec![five_hour(f64::NAN, at(100))],
                Vec::new(),
            ))
            .expect("nan");
        assert_eq!(store.samples(&k, Timestamp::MIN).len(), 1);
    }

    #[test]
    fn balances_store_every_change_and_otherwise_every_five_minutes() {
        let dir = TempDir::new("balance");
        let mut store = HistoryStore::open(&dir.0, t0()).expect("open");
        let k = key(ProviderId::Meshy, MeterId::CREDITS);
        for (m, amount) in [(0, 100.0), (1, 100.0), (2, 90.0), (3, 90.0), (8, 90.0)] {
            store
                .record(&snapshot(at(m), Vec::new(), vec![credits(amount)]))
                .expect("record");
        }
        assert_eq!(
            times(store.samples(&k, Timestamp::MIN)),
            vec![at(0), at(2), at(8)],
            "1 was replaced by 2 (a change), 3 by 8 (the 5-minute step)"
        );
        let file = dir.file("meshy.default.credits.jsonl");
        assert_eq!(lines(&file).len(), 3, "0, 2 and 8");
        let reopened = HistoryStore::open(&dir.0, at(9)).expect("reopen");
        let samples = reopened.samples(&k, Timestamp::MIN);
        assert_eq!(times(samples), vec![at(0), at(2), at(8)]);
        assert_eq!(samples[2].value, 90.0);
    }

    #[test]
    fn unreadable_lines_are_skipped_and_a_cut_line_does_not_spread() {
        let dir = TempDir::new("corrupt");
        let file = dir.file("claude.default.five_hour.jsonl");
        fs::create_dir_all(&dir.0).expect("dir");
        let good = |m: i64| {
            serde_json::to_string(&Sample::window(at(m), 0.5, Some(at(100)))).expect("json")
        };
        let long = "x".repeat(10_000);
        let text = format!(
            "{}\nnot json\n{long}\n{}\n{{\"t\":\"2026-10-10T",
            good(0),
            good(5)
        );
        fs::write(&file, text).expect("write");
        let k = key(ProviderId::Claude, MeterId::FIVE_HOUR);
        let mut store = HistoryStore::open(&dir.0, at(9)).expect("open");
        assert_eq!(times(store.samples(&k, Timestamp::MIN)), vec![at(0), at(5)]);
        store
            .record(&snapshot(at(10), vec![five_hour(0.6, at(100))], Vec::new()))
            .expect("record");
        let reopened = HistoryStore::open(&dir.0, at(11)).expect("reopen");
        assert_eq!(
            times(reopened.samples(&k, Timestamp::MIN)),
            vec![at(0), at(5), at(10)]
        );
    }

    #[test]
    fn loading_keeps_the_in_memory_span_sorts_and_ignores_the_future() {
        let dir = TempDir::new("load");
        let now = t0();
        let file = dir.file("meshy.default.credits.jsonl");
        write_samples(
            &file,
            &[
                Sample::balance(now - IN_MEMORY_SPAN - SignedDuration::from_mins(1), 500.0),
                Sample::balance(now - days(2), 300.0),
                Sample::balance(now - days(3), 400.0),
                Sample::balance(now + days(30), 1.0),
                Sample::balance(now - days(1), 200.0),
            ],
        );
        let store = HistoryStore::open(&dir.0, now).expect("open");
        let k = key(ProviderId::Meshy, MeterId::CREDITS);
        assert_eq!(
            times(store.samples(&k, Timestamp::MIN)),
            vec![now - days(3), now - days(2), now - days(1)]
        );
    }

    #[test]
    fn memory_keeps_only_what_the_statistics_need() {
        let dir = TempDir::new("trim");
        let now = t0();
        let hourly = |span_days: i64, value: &dyn Fn(i64) -> Sample| -> Vec<Sample> {
            (0..span_days * 24).rev().map(value).collect()
        };
        let h = |n: i64| now - SignedDuration::from_hours(n);
        // A 5-hour meter: an older instance, then the current one.
        write_samples(
            &dir.file("claude.default.five_hour.jsonl"),
            &[
                Sample::window(h(8), 0.5, Some(h(4))),
                Sample::window(h(3), 0.1, Some(now + SignedDuration::from_hours(2))),
                Sample::window(h(1), 0.2, Some(now + SignedDuration::from_hours(2))),
            ],
        );
        // Weekly windows (the current one resets in an hour), hourly for 25 days.
        let weekly = |n: i64| {
            let resets = h(168 * (n / 168)) + SignedDuration::from_hours(1);
            Sample::window(h(n), 0.3, Some(resets))
        };
        write_samples(
            &dir.file("claude.default.weekly.jsonl"),
            &hourly(25, &weekly),
        );
        write_samples(
            &dir.file("claude.default.weekly_fable.jsonl"),
            &hourly(25, &weekly),
        );
        // 40 days of a balance.
        write_samples(
            &dir.file("meshy.default.credits.jsonl"),
            &hourly(40, &|n| Sample::balance(h(n), 1000.0 - n as f64)),
        );

        let mut store = HistoryStore::open(&dir.0, now).expect("open");
        // The per-model meter is not in the catalogue: its kind is learned from a reading.
        let fable = meter(
            "weekly:fable",
            0.3,
            Some(weekly(0).resets_at.expect("reset")),
            WindowLen::Fixed(SignedDuration::from_hours(168)),
        );
        let later = now + SignedDuration::from_mins(1);
        store
            .record(&snapshot(later, vec![fable], Vec::new()))
            .expect("record");
        store.trim_memory(later);

        let all = |m: &str, p| times(store.samples(&key(p, m), Timestamp::MIN));
        assert_eq!(all("five_hour", ProviderId::Claude), vec![h(3), h(1)]);
        let weekly_kept = all("weekly", ProviderId::Claude);
        assert_eq!(
            weekly_kept.first(),
            Some(&h(16 * 24 - 1)),
            "16 days for F§6"
        );
        let fable_kept = all("weekly:fable", ProviderId::Claude);
        assert!(
            fable_kept.len() <= 169,
            "only the current week: {}",
            fable_kept.len()
        );
        assert!(fable_kept.first() >= Some(&h(168)));
        let credits_kept = all("credits", ProviderId::Meshy);
        assert_eq!(
            credits_kept.first(),
            Some(&h(30 * 24)),
            "30 days (R§3.6 ref)"
        );
    }

    #[test]
    fn a_service_without_a_weekly_window_keeps_its_longest_one_for_the_sparkline() {
        let dir = TempDir::new("spark");
        let now = t0();
        let h = |n: i64| now - SignedDuration::from_hours(n);
        // Codex with only a 5-hour window, a new instance every 5 hours, hourly for 20 days.
        let samples: Vec<Sample> = (0..20 * 24)
            .rev()
            .map(|n| {
                Sample::window(
                    h(n),
                    0.1,
                    Some(h(5 * (n / 5)) + SignedDuration::from_hours(1)),
                )
            })
            .collect();
        write_samples(&dir.file("codex.default.five_hour.jsonl"), &samples);
        let store = HistoryStore::open(&dir.0, now).expect("open");
        let kept =
            times(store.samples(&key(ProviderId::Codex, MeterId::FIVE_HOUR), Timestamp::MIN));
        assert_eq!(kept.first(), Some(&h(16 * 24)), "16 days for F§6");
    }

    #[test]
    fn compaction_drops_what_is_older_than_ninety_days() {
        let dir = TempDir::new("compact");
        let now = t0();
        let old_and_new = dir.file("meshy.default.credits.jsonl");
        write_samples(
            &old_and_new,
            &[
                Sample::balance(now - days(100), 9.0),
                Sample::balance(now - days(95), 8.0),
                Sample::balance(now - days(10), 7.0),
            ],
        );
        let only_old = dir.file("codex.default.weekly.jsonl");
        write_samples(&only_old, &[Sample::window(now - days(91), 0.1, None)]);
        let fresh = dir.file("claude.default.weekly.jsonl");
        write_samples(&fresh, &[Sample::window(now - days(5), 0.1, None)]);
        let fresh_before = fs::read(&fresh).expect("read");
        let unrelated = dir.file("notes.txt");
        fs::write(&unrelated, "keep me").expect("write");

        let mut store = HistoryStore::open(&dir.0, now).expect("open");
        store.compact(now).expect("compact");

        assert_eq!(lines(&old_and_new).len(), 1);
        assert!(!only_old.exists());
        assert_eq!(fs::read(&fresh).expect("read"), fresh_before);
        assert!(unrelated.exists());
        let leftovers: Vec<_> = fs::read_dir(&dir.0)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());

        // Appending after compaction still works.
        store
            .record(&snapshot(now, Vec::new(), vec![credits(6.0)]))
            .expect("record");
        assert_eq!(lines(&old_and_new).len(), 2);
    }

    #[test]
    fn a_leftover_temporary_file_is_removed_and_a_removed_folder_comes_back() {
        let dir = TempDir::new("leftover");
        let tmp = dir.file("claude.default.weekly.jsonl.tmp");
        fs::create_dir_all(&dir.0).expect("dir");
        fs::write(&tmp, "partial").expect("write");
        let mut store = HistoryStore::open(&dir.0, t0()).expect("open");
        assert!(!tmp.exists());

        fs::remove_dir_all(&dir.0).expect("remove");
        store
            .record(&snapshot(at(0), vec![five_hour(0.1, at(100))], Vec::new()))
            .expect("record recreates the folder");
        assert_eq!(lines(&dir.file("claude.default.five_hour.jsonl")).len(), 1);
    }
}
