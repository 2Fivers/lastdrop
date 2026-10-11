//! The file writer thread (ARCHITECTURE "Threads", "Persistence"): every file write the running
//! app makes — history appends and the daily history compaction, settings saves and the state
//! file — happens here, one job at a time **in the order the main thread queued them**. So an
//! append queued after a compaction lands in the compacted file, an append queued before it is
//! compacted with the rest, and a later settings save always replaces an earlier one.
//!
//! The main thread only formats and enqueues (an unbounded `std::sync::mpsc` channel: no lock,
//! no wait), so a slow or stuck disk never stalls the UI. Start-up reads (settings, state,
//! history) still run on the main thread before the event loop; the log has its own writer
//! thread ([`crate::logger`]).
//!
//! A history append that fails (disk full, file locked by a scanner) is kept and written in
//! front of the next append to the same file, in order; at most [`MAX_PENDING`] bytes per file
//! are kept, the oldest are dropped first.
//!
//! Shutdown ([`Writer::shutdown`]) writes everything queued before it and tries the failed appends
//! once more, then joins the thread, waiting at most [`SHUTDOWN_WAIT`].

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use jiff::Timestamp;

/// Failed history appends kept per file, bytes (about a day of 5-minute samples).
pub const MAX_PENDING: usize = 64 * 1024;

/// One unit of work, run in queue order.
enum Job {
    /// Append complete lines to a history file (created with its folder when missing).
    Append { path: PathBuf, text: String },
    /// Rewrite the history files of `dir` without samples older than `cutoff`
    /// ([`crate::history::compact_dir`]).
    Compact { dir: PathBuf, cutoff: Timestamp },
    /// Replace a file atomically ([`crate::persist::write_atomic`]); `what` names it in the log.
    Replace {
        path: PathBuf,
        bytes: Vec<u8>,
        what: &'static str,
    },
    /// Answer once every earlier job is done.
    Flush(Sender<()>),
    /// Hold the thread until the sender is dropped or sends (tests: a busy disk).
    #[cfg(test)]
    Block(Receiver<()>),
    /// Stop after the jobs queued before it.
    Stop,
}

/// A cheap, clonable handle that queues jobs for the writer thread. Queueing never blocks; a
/// job queued after the thread stopped is dropped.
#[derive(Clone, Debug)]
pub struct WriterHandle {
    tx: Sender<Job>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Job::Append { .. } => "Append",
            Job::Compact { .. } => "Compact",
            Job::Replace { .. } => "Replace",
            Job::Flush(_) => "Flush",
            #[cfg(test)]
            Job::Block(_) => "Block",
            Job::Stop => "Stop",
        })
    }
}

impl WriterHandle {
    /// Queues `text` (complete lines, each ending in `\n`) to be appended to `path` in one write.
    /// A file that ends in a cut line (a crash during an earlier write) gets a line break first,
    /// so the cut never spreads to the new lines.
    pub fn append(&self, path: PathBuf, text: String) {
        self.send(Job::Append { path, text });
    }

    /// Queues the daily compaction of the history folder `dir` (samples older than `cutoff` go).
    pub fn compact(&self, dir: PathBuf, cutoff: Timestamp) {
        self.send(Job::Compact { dir, cutoff });
    }

    /// Queues an atomic replace of `path` with `bytes` (settings, state). `what` names the file
    /// in the log when it fails.
    pub fn replace(&self, path: PathBuf, bytes: Vec<u8>, what: &'static str) {
        self.send(Job::Replace { path, bytes, what });
    }

    /// Waits until every job queued so far is done (tests and shutdown; never on the UI path).
    /// Returns at once when the thread is gone.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    fn send(&self, job: Job) {
        if let Err(e) = self.tx.send(job) {
            log::warn!("writer stopped; {:?} dropped", e.0);
        }
    }
}

/// How long [`Writer::shutdown`] waits for the queued writes (**not from the spec**): far more
/// than a settings save, a state save and a day's compaction take on a working disk, short enough
/// that a stuck disk (a dead network drive, a scanner holding a file) does not keep the process
/// alive without end (the app removes its tray icon and readout before it waits). Writes still
/// queued after it end with the process.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

/// The writer thread and its queue (main thread; [`Writer::shutdown`] joins it).
pub struct Writer {
    handle: WriterHandle,
    thread: RefCell<Option<JoinHandle<()>>>,
    /// Closed by the thread when it ends (the sender drops), so shutdown can wait with a limit.
    ended: Receiver<()>,
}

impl Writer {
    /// Starts the thread.
    pub fn start() -> io::Result<Writer> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ending, ended) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("lastdrop-writer".to_owned())
            .spawn(move || {
                run(&rx);
                drop(ending);
            })?;
        Ok(Writer {
            handle: WriterHandle { tx },
            thread: RefCell::new(Some(thread)),
            ended,
        })
    }

    /// A handle that queues jobs.
    pub fn handle(&self) -> WriterHandle {
        self.handle.clone()
    }

    /// Writes every job queued so far, then stops and joins the thread, waiting at most
    /// [`SHUTDOWN_WAIT`]. Later jobs are dropped.
    pub fn shutdown(&self) {
        self.shutdown_within(SHUTDOWN_WAIT);
    }

    /// [`Writer::shutdown`] waiting at most `wait`: a thread still writing after it is left to
    /// end with the process, and the log says so.
    pub fn shutdown_within(&self, wait: Duration) {
        let Some(thread) = self.thread.borrow_mut().take() else {
            return;
        };
        let _ = self.handle.tx.send(Job::Stop);
        match self.ended.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) => {
                log::warn!(
                    "the writer is still busy after {} s; writes not done yet are lost",
                    wait.as_secs()
                );
            }
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                if thread.join().is_err() {
                    log::error!("the writer thread panicked");
                }
            }
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The thread: one job at a time, in order.
fn run(rx: &Receiver<Job>) {
    let mut pending: BTreeMap<PathBuf, String> = BTreeMap::new();
    while let Ok(job) = rx.recv() {
        match job {
            Job::Append { path, text } => append(&mut pending, path, text),
            Job::Compact { dir, cutoff } => {
                if let Err(e) = crate::history::compact_dir(&dir, cutoff) {
                    log::warn!("history compaction: {e}");
                }
            }
            Job::Replace { path, bytes, what } => {
                if let Err(e) = crate::persist::write_atomic(&path, &bytes) {
                    log::error!("cannot save {what}: {e}");
                }
            }
            Job::Flush(ack) => {
                let _ = ack.send(());
            }
            #[cfg(test)]
            Job::Block(until) => {
                let _ = until.recv();
            }
            Job::Stop => break,
        }
    }
    // Appends that failed are tried once more before the thread ends; what still fails is lost,
    // and the log says how much.
    for (path, text) in pending {
        if let Err(e) = append_lines(&path, &text) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            log::warn!(
                "history: {} line(s) for {} not written at exit: {e}",
                text.lines().count(),
                name.unwrap_or_default()
            );
        }
    }
}

/// An append, with the failed earlier appends to the same file in front of it.
fn append(pending: &mut BTreeMap<PathBuf, String>, path: PathBuf, text: String) {
    let text = match pending.remove(&path) {
        Some(mut earlier) => {
            earlier.push_str(&text);
            earlier
        }
        None => text,
    };
    if let Err(e) = append_lines(&path, &text) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        log::warn!(
            "history: cannot append to {}: {e}",
            name.unwrap_or_default()
        );
        pending.insert(path, keep_newest(text, MAX_PENDING));
    }
}

/// `text` without its oldest lines, so that it is at most `max` bytes.
fn keep_newest(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    // `cut ≥ 1`; every slice starts right after an ASCII line break, so on a char boundary.
    let cut = text.len() - max;
    let bytes = text.as_bytes();
    if bytes[cut - 1] == b'\n' {
        return text[cut..].to_owned();
    }
    match bytes[cut..].iter().position(|&b| b == b'\n') {
        Some(n) => text[cut + n + 1..].to_owned(),
        None => String::new(),
    }
}

/// Appends `text` to `path` in one write (the folder and file are created when missing); when the
/// file ends in a cut line, a line break goes first.
pub(crate) fn append_lines(path: &Path, text: &str) -> io::Result<()> {
    let open = || {
        OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)
    };
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
    let cut = if file.metadata()?.len() == 0 {
        false
    } else {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        last[0] != b'\n'
    };
    if cut {
        let mut joined = String::with_capacity(text.len() + 1);
        joined.push('\n');
        joined.push_str(text);
        file.write_all(joined.as_bytes())
    } else {
        file.write_all(text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;

    use jiff::{SignedDuration, Timestamp};
    use lastdrop_core::stats::Sample;

    use super::{Job, Writer, keep_newest};

    /// A folder of the test's own under %TEMP%, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "lastdrop-writer-test-{}-{n}-{tag}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn now() -> Timestamp {
        "2026-10-10T10:42:00Z".parse().expect("timestamp")
    }

    fn line(days_ago: i64, value: f64) -> String {
        let t = now() - SignedDuration::from_hours(24 * days_ago);
        serde_json::to_string(&Sample::balance(t, value)).expect("json") + "\n"
    }

    fn values(path: &std::path::Path) -> Vec<f64> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Sample>(l).expect("sample").value)
            .collect()
    }

    #[test]
    fn appends_and_compaction_run_in_queue_order() {
        let dir = TempDir::new("order");
        let file = dir.0.join("meshy.default.credits.jsonl");
        fs::write(&file, line(100, 1.0) + &line(10, 2.0)).expect("seed");
        let writer = Writer::start().expect("writer");
        let w = writer.handle();
        // Queued before the compaction: an old sample (gone after it) and a recent one.
        w.append(file.clone(), line(95, 3.0));
        w.append(file.clone(), line(5, 4.0));
        w.compact(dir.0.clone(), now() - SignedDuration::from_hours(90 * 24));
        // Queued after it: kept even though it is old, because it was written after the
        // compaction ran.
        w.append(file.clone(), line(99, 5.0));
        w.append(file.clone(), line(1, 6.0));
        w.flush();
        assert_eq!(values(&file), vec![2.0, 4.0, 5.0, 6.0]);
        writer.shutdown();
    }

    #[test]
    fn a_later_replace_wins_and_shutdown_writes_everything_queued() {
        let dir = TempDir::new("replace");
        let settings = dir.0.join("settings.json");
        let history = dir.0.join("claude.default.weekly.jsonl");
        let writer = Writer::start().expect("writer");
        let w = writer.handle();
        for n in 0..50u32 {
            w.replace(
                settings.clone(),
                format!("{{\"n\": {n}}}\n").into_bytes(),
                "test",
            );
            w.append(history.clone(), line(0, f64::from(n)));
        }
        // No flush: shutdown itself drains the queue before the thread ends (a hundred synced
        // writes, so with more time than the app's few writes at exit get).
        writer.shutdown_within(std::time::Duration::from_secs(120));
        assert_eq!(
            fs::read_to_string(&settings).expect("settings"),
            "{\"n\": 49}\n"
        );
        let expected: Vec<f64> = (0..50).map(f64::from).collect();
        assert_eq!(values(&history), expected);
        // Jobs after shutdown are dropped, not run (and do not hang).
        w.append(history.clone(), line(0, 99.0));
        w.flush();
        assert_eq!(values(&history).len(), 50);
    }

    #[test]
    fn queueing_never_waits_for_a_busy_disk() {
        let dir = TempDir::new("busy");
        let file = dir.0.join("codex.default.weekly.jsonl");
        let writer = Writer::start().expect("writer");
        let w = writer.handle();
        let (release, until) = mpsc::channel();
        w.send(Job::Block(until));
        // The thread is stuck; the main thread still queues at once, and nothing is written yet.
        for n in 0..1000u32 {
            w.append(file.clone(), line(0, f64::from(n)));
        }
        assert!(!file.exists());
        release.send(()).expect("release");
        w.flush();
        assert_eq!(values(&file).len(), 1000);
        writer.shutdown();
    }

    #[test]
    fn a_failed_append_is_written_in_front_of_the_next_one() {
        let dir = TempDir::new("retry");
        let file = dir.0.join("claude.default.five_hour.jsonl");
        // A folder where the file should be: the append fails.
        fs::create_dir_all(&file).expect("blocking folder");
        let writer = Writer::start().expect("writer");
        let w = writer.handle();
        w.append(file.clone(), line(0, 1.0));
        w.flush();
        assert!(file.is_dir());
        fs::remove_dir(&file).expect("unblock");
        w.append(file.clone(), line(0, 2.0));
        w.flush();
        assert_eq!(values(&file), vec![1.0, 2.0]);
        writer.shutdown();
    }

    #[test]
    fn a_failed_append_is_tried_once_more_at_shutdown() {
        let dir = TempDir::new("exit");
        let file = dir.0.join("codex.default.five_hour.jsonl");
        fs::create_dir_all(&file).expect("blocking folder");
        let writer = Writer::start().expect("writer");
        writer.handle().append(file.clone(), line(0, 7.0));
        writer.handle().flush();
        assert!(file.is_dir(), "the append failed");
        fs::remove_dir(&file).expect("unblock");
        // No later append to that file: shutdown writes the kept line itself.
        writer.shutdown();
        assert_eq!(values(&file), vec![7.0]);
    }

    #[test]
    fn shutdown_does_not_wait_for_a_stuck_disk_without_end() {
        let writer = Writer::start().expect("writer");
        let (release, until) = mpsc::channel();
        writer.handle().send(Job::Block(until));
        let started = std::time::Instant::now();
        writer.shutdown_within(std::time::Duration::from_millis(100));
        let waited = started.elapsed();
        assert!(
            waited < std::time::Duration::from_secs(2),
            "waited {waited:?} for a stuck writer"
        );
        // The stuck write ends later; the thread then ends on its own.
        drop(release);
    }

    #[test]
    fn a_cut_last_line_gets_a_line_break_first() {
        let dir = TempDir::new("cut");
        let file = dir.0.join("claude.default.weekly.jsonl");
        fs::write(&file, line(1, 1.0) + "{\"t\":\"2026-10").expect("seed");
        let writer = Writer::start().expect("writer");
        writer.handle().append(file.clone(), line(0, 2.0));
        writer.shutdown();
        let text = fs::read_to_string(&file).expect("read");
        let last = text.lines().last().expect("a line");
        assert_eq!(
            serde_json::from_str::<Sample>(last).expect("sample").value,
            2.0
        );
    }

    #[test]
    fn pending_text_keeps_the_newest_whole_lines() {
        assert_eq!(keep_newest("a\nbb\nccc\n".to_owned(), 100), "a\nbb\nccc\n");
        assert_eq!(keep_newest("a\nbb\nccc\n".to_owned(), 5), "ccc\n");
        assert_eq!(keep_newest("a\nbb\nccc\n".to_owned(), 7), "bb\nccc\n");
        assert_eq!(keep_newest("abcdef".to_owned(), 3), "");
    }
}
