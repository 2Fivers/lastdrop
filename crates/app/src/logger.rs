//! A tiny file logger for the `log` facade (no heavy logging stacks): one line per record to
//! `%LOCALAPPDATA%\Lastdrop\logs\lastdrop.log`, rotated by size, local only, never sent anywhere.
//!
//! **Never log a secret** (ARCHITECTURE): callers log no tokens, keys, cookies, account ids or
//! e-mail addresses; [`redact`] masks known shapes as a second line of defence on every line.
//!
//! Threading: any thread may log, **including the readout thread**, which must never wait on a
//! lock held by another thread or do I/O (ARCHITECTURE "Threads"). So `log` calls only format,
//! redact and push the line into an unbounded `std::sync::mpsc` channel (no file access, no lock
//! that another thread holds across I/O); one writer thread owns the file, writes, rotates and
//! flushes. The writer is joined at shutdown ([`shutdown`]) after it has written every queued
//! line.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

/// Rotate when the current file reaches 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// Files kept: `lastdrop.log`, `lastdrop.1.log`, `lastdrop.2.log`.
pub const KEEP_FILES: usize = 3;

/// The log file name.
pub const FILE_NAME: &str = "lastdrop.log";

/// What the writer thread receives.
enum Line {
    /// A formatted, redacted line (without the line ending).
    Text(String),
    /// Write everything queued, then stop.
    Stop,
}

/// The writer thread, joined by [`shutdown`].
static WRITER: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
/// The queue, to send [`Line::Stop`] at shutdown.
static QUEUE: OnceLock<Sender<Line>> = OnceLock::new();

/// The `log` implementation: format, redact, queue.
struct FileLogger {
    queue: Sender<Line>,
    level: log::LevelFilter,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}",
            jiff::Timestamp::now(),
            record.level(),
            record.target(),
            record.args()
        );
        // Never more than one physical line per record.
        let line = line.replace(['\r', '\n'], " ");
        let line = redact(&line).into_owned();
        // A closed queue (after shutdown) drops the line.
        let _ = self.queue.send(Line::Text(line));
    }

    fn flush(&self) {}
}

/// Installs the logger at `level` (`log::set_boxed_logger`) and starts its writer thread, which
/// writes into `dir`.
pub fn init(dir: &Path, level: log::LevelFilter) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = LogFile::open(dir.to_path_buf())?;
    let (tx, rx) = mpsc::channel::<Line>();
    let handle = std::thread::Builder::new()
        .name("lastdrop-log".to_owned())
        .spawn(move || file.run(&rx))?;
    let _ = QUEUE.set(tx.clone());
    let _ = WRITER.set(Mutex::new(Some(handle)));
    log::set_boxed_logger(Box::new(FileLogger { queue: tx, level }))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    log::set_max_level(level);
    Ok(())
}

/// Writes every queued line and stops the writer thread. Lines logged afterwards are dropped.
pub fn shutdown() {
    if let Some(queue) = QUEUE.get() {
        let _ = queue.send(Line::Stop);
    }
    let handle = WRITER
        .get()
        .and_then(|w| w.lock().ok().and_then(|mut h| h.take()));
    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

/// The open log file and its rotation.
struct LogFile {
    dir: PathBuf,
    out: BufWriter<File>,
    size: u64,
}

impl LogFile {
    fn open(dir: PathBuf) -> std::io::Result<LogFile> {
        let path = dir.join(FILE_NAME);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(LogFile {
            dir,
            out: BufWriter::new(file),
            size,
        })
    }

    /// The writer loop: block for a line, write it and whatever else is queued, flush.
    fn run(&mut self, rx: &Receiver<Line>) {
        while let Ok(first) = rx.recv() {
            let mut stop = !self.take(first);
            while !stop {
                match rx.try_recv() {
                    Ok(next) => stop = !self.take(next),
                    Err(_) => break,
                }
            }
            let _ = self.out.flush();
            if stop {
                break;
            }
        }
        let _ = self.out.flush();
    }

    /// Writes one line; `false` for [`Line::Stop`].
    fn take(&mut self, line: Line) -> bool {
        match line {
            Line::Stop => false,
            Line::Text(text) => {
                if self.size >= MAX_FILE_BYTES
                    && let Err(e) = self.rotate()
                {
                    // Nowhere to report it but the file itself.
                    let _ = writeln!(self.out, "log rotation failed: {e}");
                }
                if writeln!(self.out, "{text}").is_ok() {
                    self.size += text.len() as u64 + 1;
                }
                true
            }
        }
    }

    /// `lastdrop.1.log` → `lastdrop.2.log`, `lastdrop.log` → `lastdrop.1.log`, new file.
    fn rotate(&mut self) -> std::io::Result<()> {
        self.out.flush()?;
        let name = |n: usize| {
            if n == 0 {
                self.dir.join(FILE_NAME)
            } else {
                self.dir.join(format!("lastdrop.{n}.log"))
            }
        };
        for n in (1..KEEP_FILES).rev() {
            let from = name(n - 1);
            if from.exists() {
                // Windows refuses to rename onto an existing file.
                let _ = std::fs::remove_file(name(n));
                std::fs::rename(&from, name(n))?;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(name(0))?;
        self.out = BufWriter::new(file);
        self.size = 0;
        Ok(())
    }
}

/// The mask that replaces a secret.
const MASK: &str = "[redacted]";
/// The mask that replaces an e-mail address.
const EMAIL_MASK: &str = "[email]";
/// Runs of this many token characters or more are masked whatever they look like.
const LONG_RUN: usize = 32;

/// Key names whose values are secrets (`api_key=…`, `"accessToken": "…"`), compared ASCII
/// case-insensitively with `-` and `_` ignored.
const SECRET_KEYS: [&str; 12] = [
    "apikey",
    "accesstoken",
    "refreshtoken",
    "idtoken",
    "token",
    "secret",
    "clientsecret",
    "password",
    "cookie",
    "sessionkey",
    "xapikey",
    "authorization",
];

/// Characters of a token: what keys, JWTs and base64/hex runs are made of.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '=' | '~')
}

/// Masks known secret shapes in `line`: `Bearer <token>`, `sk-ant-…`, JWT-like `eyJ…` runs,
/// `api_key=…`/`"accessToken": "…"` values, e-mail addresses, and long hex/base64 runs (≥ 32
/// characters). Returns the line unchanged (borrowed) when nothing matched.
pub fn redact(line: &str) -> Cow<'_, str> {
    let mut out = String::with_capacity(line.len());
    let mut changed = false;
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        // `Bearer <token>` / `Basic <token>`: keep the scheme, mask the credential.
        if let Some(scheme_len) = auth_scheme(rest)
            && (i == 0 || !is_token_char(char_before(line, i)))
        {
            let after = &rest[scheme_len..];
            let token_len = token_len(after);
            if token_len > 0 {
                out.push_str(&rest[..scheme_len]);
                out.push_str(MASK);
                i += scheme_len + token_len;
                changed = true;
                continue;
            }
        }
        // `key=value`, `key: value`, `"key": "value"`.
        if let Some((keep, value_len)) = secret_assignment(rest)
            && (i == 0 || !is_token_char(char_before(line, i)))
            && value_len > 0
        {
            out.push_str(&rest[..keep]);
            out.push_str(MASK);
            i += keep + value_len;
            changed = true;
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        if is_token_char(c) || c == '@' {
            // A whole run of token characters (and `@` for e-mail addresses).
            let run_len = rest
                .char_indices()
                .find(|(_, ch)| !(is_token_char(*ch) || *ch == '@'))
                .map_or(rest.len(), |(n, _)| n);
            let run = &rest[..run_len];
            if let Some(masked) = mask_run(run) {
                out.push_str(&masked);
                changed = true;
            } else {
                out.push_str(run);
            }
            i += run_len;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(line)
    }
}

/// The character before byte index `i` of `s` (a space at the start).
fn char_before(s: &str, i: usize) -> char {
    s[..i].chars().next_back().unwrap_or(' ')
}

/// The length of a token at the start of `s`.
fn token_len(s: &str) -> usize {
    s.char_indices()
        .find(|(_, c)| !is_token_char(*c))
        .map_or(s.len(), |(n, _)| n)
}

/// `Bearer ` or `Basic ` (any case) at the start of `s`: its length including the spaces.
fn auth_scheme(s: &str) -> Option<usize> {
    for scheme in ["bearer", "basic"] {
        let n = scheme.len();
        if s.len() > n
            && s.is_char_boundary(n)
            && s[..n].eq_ignore_ascii_case(scheme)
            && s[n..].starts_with(' ')
        {
            let spaces = s[n..].len() - s[n..].trim_start_matches(' ').len();
            return Some(n + spaces);
        }
    }
    None
}

/// A secret key assignment at the start of `s`: `(bytes to keep: key, separator and opening
/// quote, value length)`. The value ends at whitespace, a quote, `&`, `,`, `;`, `}` or `]`.
fn secret_assignment(s: &str) -> Option<(usize, usize)> {
    let quoted_key = s.starts_with('"');
    let key_start = usize::from(quoted_key);
    let key_len = s[key_start..]
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
        .map_or(s.len() - key_start, |(n, _)| n);
    if key_len == 0 {
        return None;
    }
    let key: String = s[key_start..key_start + key_len]
        .chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect();
    if !SECRET_KEYS.contains(&key.as_str()) {
        return None;
    }
    let mut j = key_start + key_len;
    if quoted_key {
        if !s[j..].starts_with('"') {
            return None;
        }
        j += 1;
    }
    let after_key = &s[j..];
    let sep = after_key.trim_start_matches(' ');
    let sep_char = sep.chars().next()?;
    if sep_char != '=' && sep_char != ':' {
        return None;
    }
    j += after_key.len() - sep.len() + 1;
    j += s[j..].len() - s[j..].trim_start_matches(' ').len();
    if s[j..].starts_with('"') || s[j..].starts_with('\'') {
        j += 1;
    }
    // `Authorization: Bearer x` is left to the scheme rule.
    if auth_scheme(&s[j..]).is_some() {
        return None;
    }
    let value_len = s[j..]
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '"' | '\'' | '&' | ',' | ';' | '}' | ']'))
        .map_or(s.len() - j, |(n, _)| n);
    Some((j, value_len))
}

/// Masks a run of token characters (and `@`) when it is secret-shaped; `None` keeps it.
fn mask_run(run: &str) -> Option<String> {
    // An e-mail address: local@domain.tld.
    if let Some(at) = run.find('@') {
        let (local, domain) = (&run[..at], &run[at + 1..]);
        let domain = domain.trim_end_matches('.');
        if !local.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && domain
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            // A sentence's full stop after the address stays.
            let trailing = &run[run.trim_end_matches('.').len()..];
            return Some(format!("{EMAIL_MASK}{trailing}"));
        }
    }
    let lower = run.to_ascii_lowercase();
    if lower.starts_with("sk-") && run.len() >= 20 {
        return Some(MASK.to_owned());
    }
    if run.starts_with("eyJ") && run.len() >= 20 {
        return Some(MASK.to_owned());
    }
    // Long runs of key material; path-like and dotted runs (file names, versions, hosts) are
    // judged by their longest dot/slash-free piece.
    let longest = run.split(['.', '/', '@']).map(str::len).max().unwrap_or(0);
    if longest >= LONG_RUN && run.chars().any(|c| c.is_ascii_digit()) {
        return Some(MASK.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented secret-shaped strings only.
    #[test]
    fn masks_known_shapes() {
        assert_eq!(
            redact("Authorization: Bearer test-token-not-real-0123456789"),
            "Authorization: Bearer [redacted]"
        );
        assert!(!redact("key sk-ant-test-0000000000000000000000000000").contains("sk-ant-test"));
        assert!(
            !redact("user tester@example.invalid signed in").contains("tester@example.invalid")
        );
        assert_eq!(
            redact("Claude 5-hour 42% at 14:56"),
            "Claude 5-hour 42% at 14:56"
        );
    }

    #[test]
    fn masks_assignments_jwts_and_long_runs() {
        assert_eq!(redact("api_key=abc123&x=1"), "api_key=[redacted]&x=1");
        assert_eq!(
            redact(r#"{"accessToken": "not-a-real-token", "plan": "max"}"#),
            r#"{"accessToken": "[redacted]", "plan": "max"}"#
        );
        assert_eq!(redact("x-api-key: invented42"), "x-api-key: [redacted]");
        assert_eq!(
            redact("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0In0.c2lnbmF0dXJl end"),
            "jwt [redacted] end"
        );
        assert_eq!(
            redact("blob 0123456789abcdef0123456789abcdef01 tail"),
            "blob [redacted] tail"
        );
        assert_eq!(
            redact("mail Tester.Name+tag@sub.example.invalid."),
            "mail [email]."
        );
    }

    #[test]
    fn leaves_ordinary_lines_alone() {
        for line in [
            r"settings: Loaded from C:\Users\tester\AppData\Roaming\Lastdrop\settings.json",
            "Claude/default: read from ClaudeOAuthUsage",
            "Codex weekly 61% used, resets Mon 09:00",
            "HTTP 503 from api.example.invalid, retry after 120 s",
            "token count 42 of 100",
            "rollout-2026-10-10T12-00-00.jsonl",
        ] {
            assert!(matches!(redact(line), Cow::Borrowed(_)), "{line}");
        }
    }

    #[test]
    fn writes_rotates_and_stops() {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-log-test-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut file = LogFile::open(dir.clone()).expect("open");
        let (tx, rx) = mpsc::channel();
        let long = "x".repeat(1000);
        // Enough to rotate twice.
        for _ in 0..(2 * MAX_FILE_BYTES as usize / 1000 + 10) {
            tx.send(Line::Text(long.clone())).expect("send");
        }
        tx.send(Line::Stop).expect("send");
        file.run(&rx);
        assert!(dir.join(FILE_NAME).exists());
        assert!(dir.join("lastdrop.1.log").exists());
        assert!(dir.join("lastdrop.2.log").exists());
        assert!(
            !dir.join("lastdrop.3.log").exists(),
            "only three files are kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
