//! The requests Claude Code and Codex logged on this PC, for tokens and cost per day and model
//! (docs/design/tokens-spec.md §1). Read-only through [`Fs`]; only the usage fields are kept, never
//! a prompt, an answer, a file name or a project path, and nothing is logged about them.
//!
//! The logs can be gigabytes (tokens-spec §4): files are read on a few threads at once, each line
//! is filtered by a byte search (`memchr::memmem`) before it is parsed, and parsed into a struct
//! that keeps only the fields used.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use jiff::Timestamp;
use lastdrop_core::model::fnv1a64;
use lastdrop_core::tokens::{Request, TokenLog};
use memchr::memmem::Finder;
use serde::Deserialize;

use crate::io::{DirEntry, Fs};

/// Folders below `<config_dir>\projects` that are searched: a project, a session, its
/// `subagents`, their `workflows` and each workflow's folder, with room to spare.
const CLAUDE_DEPTH: usize = 8;

/// Claude Code's placeholder model for messages it makes up itself.
const SYNTHETIC: &str = "<synthetic>";

/// The model of Codex requests before any `turn_context` names one.
pub const UNKNOWN_MODEL: &str = "unknown";

/// The largest zstd window a compressed rollout may ask for.
const MAX_ZSTD_WINDOW: u64 = 64 << 20;

/// Files read at once.
const MAX_READERS: usize = 4;

/// The `*.jsonl` files below `dir` up to `depth` folders deep, changed at or after `since`
/// (folders are always searched: their time says nothing about the files in them).
fn files_below(fs: &dyn Fs, dir: &Path, depth: usize, since: Timestamp, out: &mut Vec<DirEntry>) {
    let Ok(entries) = fs.list(dir) else {
        return;
    };
    for entry in entries {
        if entry.meta.is_dir {
            if depth > 0 {
                files_below(fs, &entry.path, depth - 1, since, out);
            }
        } else if is_log(&entry.path) && entry.meta.modified.is_none_or(|m| m >= since) {
            out.push(entry);
        }
    }
}

/// A `.jsonl` file or its compressed `.jsonl.zst`.
fn is_log(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    name.ends_with(".jsonl") || name.ends_with(".jsonl.zst")
}

/// Calls `f` with every line of the file (without its line break), decompressing `.zst`; stops
/// quietly at the first read error (a file being written or rotated).
fn each_line(fs: &dyn Fs, path: &Path, mut f: impl FnMut(&[u8])) {
    let Ok(reader) = fs.open(path, u64::MAX) else {
        return;
    };
    let compressed = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zst"));
    let reader: Box<dyn Read> = if compressed {
        match ruzstd::decoding::StreamingDecoder::new_with_max_window_size(reader, MAX_ZSTD_WINDOW)
        {
            Ok(decoder) => Box::new(decoder),
            Err(_) => return,
        }
    } else {
        reader
    };
    let mut reader = BufReader::with_capacity(256 << 10, reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => f(line.trim_ascii()),
        }
    }
}

/// `f` of every file, on up to [`MAX_READERS`] threads, in the files' order.
fn read_all<T: Send>(files: &[DirEntry], f: impl Fn(&DirEntry) -> T + Sync) -> Vec<T> {
    let readers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_READERS)
        .min(files.len());
    if readers <= 1 {
        return files.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<T>>> = Mutex::new(files.iter().map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..readers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else {
                        break;
                    };
                    let out = f(file);
                    let mut results = results.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(slot) = results.get_mut(i) {
                        *slot = Some(out);
                    }
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .into_iter()
        .flatten()
        .collect()
}

fn timestamp(s: Option<&str>) -> Option<Timestamp> {
    s?.parse().ok()
}

/// What one file held: its models and its requests (models are indices into its own list), with
/// each request's key for telling copies apart.
#[derive(Default)]
struct FileLog {
    log: TokenLog,
    keys: Vec<Option<u64>>,
}

impl FileLog {
    fn push(&mut self, model: &str, request: Request, key: Option<u64>) {
        if let Some(model) = self.log.model_index(model) {
            self.log.requests.push(Request { model, ..request });
            self.keys.push(key);
        }
    }
}

// ---- Claude Code ----------------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClaudeLine<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<&'a str>,
    #[serde(borrow, default)]
    timestamp: Option<&'a str>,
    #[serde(rename = "requestId", borrow, default)]
    request_id: Option<&'a str>,
    #[serde(borrow, default)]
    message: Option<ClaudeMessage<'a>>,
}

#[derive(Deserialize)]
struct ClaudeMessage<'a> {
    #[serde(borrow, default)]
    id: Option<&'a str>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize, Default)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize, Default)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<u64>,
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<u64>,
}

/// One request from a Claude Code line, with its model and its key (`message.id`, `requestId`).
fn claude_request(line: &[u8]) -> Option<(Request, String, Option<u64>)> {
    let parsed: ClaudeLine<'_> = serde_json::from_slice(line).ok()?;
    if parsed.kind != Some("assistant") {
        return None;
    }
    let at = timestamp(parsed.timestamp)?;
    let message = parsed.message?;
    let model = message.model.filter(|m| !m.is_empty() && m != SYNTHETIC)?;
    let usage = message.usage?;
    let writes = usage.cache_creation_input_tokens.unwrap_or(0);
    let (w5, w1) = match usage.cache_creation {
        Some(c)
            if c.ephemeral_5m_input_tokens.is_some() || c.ephemeral_1h_input_tokens.is_some() =>
        {
            let w1 = c.ephemeral_1h_input_tokens.unwrap_or(0);
            (
                c.ephemeral_5m_input_tokens
                    .unwrap_or(writes.saturating_sub(w1)),
                w1,
            )
        }
        _ => (writes, 0),
    };
    let request = Request {
        at,
        model: 0,
        input: usage.input_tokens.unwrap_or(0),
        output: usage.output_tokens.unwrap_or(0),
        cache_read: usage.cache_read_input_tokens.unwrap_or(0),
        cache_write_5m: w5,
        cache_write_1h: w1,
    };
    if request.total() == 0 {
        return None;
    }
    let key = message
        .id
        .map(|id| fnv1a64(&[id, parsed.request_id.unwrap_or_default()]));
    Some((request, model, key))
}

/// The requests of one Claude Code transcript. Only lines with both keys are parsed (a quote
/// inside a JSON string is escaped, so these searches only match keys).
fn read_transcript(fs: &dyn Fs, path: &Path) -> FileLog {
    let assistant = Finder::new(b"\"type\":\"assistant\"");
    let usage = Finder::new(b"\"usage\":");
    let mut out = FileLog::default();
    each_line(fs, path, |line| {
        if usage.find(line).is_none() || assistant.find(line).is_none() {
            return;
        }
        if let Some((request, model, key)) = claude_request(line) {
            out.push(&model, request, key);
        }
    });
    out
}

/// The requests Claude Code logged in `config_dir` (`projects\**\*.jsonl`) in files changed at or
/// after `since`, one per `message.id` and `requestId` (tokens-spec §1).
pub fn read_claude(fs: &dyn Fs, config_dir: &Path, since: Timestamp) -> TokenLog {
    let mut files = Vec::new();
    files_below(
        fs,
        &config_dir.join("projects"),
        CLAUDE_DEPTH,
        since,
        &mut files,
    );
    let per_file = read_all(&files, |file| read_transcript(fs, &file.path));
    let mut log = TokenLog {
        files: files.len(),
        ..TokenLog::default()
    };
    let mut seen: HashMap<u64, usize> = HashMap::new();
    for file in per_file {
        for (request, key) in file.log.requests.iter().zip(file.keys) {
            let model = file
                .log
                .models
                .get(usize::from(request.model))
                .and_then(|m| log.model_index(m));
            let Some(model) = model else {
                continue;
            };
            let request = Request { model, ..*request };
            match key.and_then(|k| seen.get(&k).copied()) {
                // A copy: the line with the most tokens wins, at the time of the first.
                Some(i) => {
                    if let Some(old) = log.requests.get_mut(i) {
                        let at = old.at.min(request.at);
                        if request.total() > old.total() {
                            *old = request;
                        }
                        old.at = at;
                    }
                }
                None => {
                    if let Some(key) = key {
                        seen.insert(key, log.requests.len());
                    }
                    log.requests.push(request);
                }
            }
        }
    }
    log
}

// ---- Codex ----------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct CodexLine<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<&'a str>,
    #[serde(borrow, default)]
    timestamp: Option<&'a str>,
    #[serde(borrow, default)]
    payload: Option<CodexPayload<'a>>,
}

#[derive(Deserialize)]
struct CodexPayload<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<&'a str>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    info: Option<CodexInfo>,
}

#[derive(Deserialize)]
struct CodexInfo {
    #[serde(default)]
    total_token_usage: Option<CodexUsage>,
    #[serde(default)]
    last_token_usage: Option<CodexUsage>,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl CodexUsage {
    fn minus(self, earlier: CodexUsage) -> CodexUsage {
        CodexUsage {
            input_tokens: self.input_tokens.saturating_sub(earlier.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(earlier.cached_input_tokens),
            output_tokens: self.output_tokens.saturating_sub(earlier.output_tokens),
        }
    }
}

/// The requests of one rollout file.
fn read_rollout(fs: &dyn Fs, path: &Path) -> FileLog {
    let turn_context = Finder::new(b"turn_context");
    let token_count = Finder::new(b"token_count");
    let mut out = FileLog::default();
    let mut model: Option<String> = None;
    let mut last_total: Option<CodexUsage> = None;
    each_line(fs, path, |line| {
        if turn_context.find(line).is_none() && token_count.find(line).is_none() {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<CodexLine<'_>>(line) else {
            return;
        };
        let Some(payload) = parsed.payload else {
            return;
        };
        match parsed.kind {
            Some("turn_context") => {
                if let Some(m) = payload.model.filter(|m| !m.is_empty()) {
                    model = Some(m);
                }
            }
            Some("event_msg") if payload.kind == Some("token_count") => {
                let Some(info) = payload.info else {
                    return;
                };
                let Some(at) = timestamp(parsed.timestamp) else {
                    return;
                };
                let usage = match (info.total_token_usage, info.last_token_usage) {
                    (Some(total), _) if last_total == Some(total) => return,
                    (Some(total), last) => {
                        let previous = last_total.replace(total);
                        last.unwrap_or_else(|| total.minus(previous.unwrap_or_default()))
                    }
                    (None, Some(last)) => last,
                    (None, None) => return,
                };
                let cached = usage.cached_input_tokens.min(usage.input_tokens);
                let request = Request {
                    at,
                    model: 0,
                    input: usage.input_tokens - cached,
                    output: usage.output_tokens,
                    cache_read: cached,
                    cache_write_5m: 0,
                    cache_write_1h: 0,
                };
                if request.total() > 0 {
                    out.push(model.as_deref().unwrap_or(UNKNOWN_MODEL), request, None);
                }
            }
            _ => {}
        }
    });
    out
}

/// The requests Codex logged in `codex_home` (`sessions\YYYY\MM\DD` and `archived_sessions`,
/// `rollout-*.jsonl` and `.jsonl.zst`) in files changed at or after `since` (tokens-spec §1).
pub fn read_codex(fs: &dyn Fs, codex_home: &Path, since: Timestamp) -> TokenLog {
    let mut files = Vec::new();
    files_below(
        fs,
        &codex_home.join(crate::codex::rollout::SESSIONS_DIR),
        3,
        since,
        &mut files,
    );
    files_below(
        fs,
        &codex_home.join("archived_sessions"),
        0,
        since,
        &mut files,
    );
    files.retain(|f| {
        f.path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().starts_with("rollout-"))
    });
    let per_file = read_all(&files, |file| read_rollout(fs, &file.path));
    let mut log = TokenLog {
        files: files.len(),
        ..TokenLog::default()
    };
    for file in per_file {
        for request in &file.log.requests {
            let model = file
                .log
                .models
                .get(usize::from(request.model))
                .and_then(|m| log.model_index(m));
            if let Some(model) = model {
                log.requests.push(Request { model, ..*request });
            }
        }
    }
    log
}
