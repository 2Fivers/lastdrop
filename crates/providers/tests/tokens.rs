//! The session-log readers for tokens and cost (tokens-spec §1), on invented logs.

use std::path::Path;

use jiff::Timestamp;
use lastdrop_core::tokens::{Request, TokenLog};
use lastdrop_providers::fakes::FakeFs;
use lastdrop_providers::tokens::{UNKNOWN_MODEL, read_claude, read_codex};

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

fn model_of<'a>(log: &'a TokenLog, r: &Request) -> &'a str {
    &log.models[usize::from(r.model)]
}

/// An invented Claude Code assistant line.
fn claude_line(at: &str, id: &str, req: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"parentUuid":"p","type":"assistant","timestamp":"{at}","requestId":"{req}","message":{{"id":"{id}","type":"message","role":"assistant","model":"{model}","content":[{{"type":"text","text":"invented answer with \"usage\" in it"}}],"usage":{usage}}}}}"#
    )
}

#[test]
fn claude_lines_are_one_request_per_message_and_request() {
    let dir = Path::new(r"C:\Users\test\.claude");
    let fs = FakeFs::new();
    let usage = r#"{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":200}}"#;
    let partial = r#"{"input_tokens":10,"output_tokens":1,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000}"#;
    let session = [
        r#"{"type":"user","timestamp":"2026-10-15T09:59:00Z","message":{"role":"user","content":"invented"}}"#.to_owned(),
        // Two content blocks of one message: the larger counts win.
        claude_line("2026-10-15T10:00:01Z", "msg_1", "req_1", "claude-opus-5-5", partial),
        claude_line("2026-10-15T10:00:02Z", "msg_1", "req_1", "claude-opus-5-5", usage),
        // A placeholder and an empty one do not count.
        claude_line("2026-10-15T10:01:00Z", "msg_2", "req_2", "<synthetic>", usage),
        claude_line(
            "2026-10-15T10:02:00Z",
            "msg_3",
            "req_3",
            "claude-haiku-5-5",
            r#"{"input_tokens":0,"output_tokens":0}"#,
        ),
        "not json at all".to_owned(),
        claude_line(
            "2026-10-15T10:03:00Z",
            "msg_4",
            "req_4",
            "claude-haiku-5-5",
            r#"{"input_tokens":7,"output_tokens":3,"cache_creation_input_tokens":40}"#,
        ),
    ]
    .join("\n");
    fs.add_file(
        dir.join(r"projects\C--work-app\11111111.jsonl"),
        session.clone(),
        ts("2026-10-15T10:05:00Z"),
    );
    // A resumed session copies msg_1 into its own file; a subagent has its own.
    let resumed = [
        claude_line(
            "2026-10-15T10:00:02Z",
            "msg_1",
            "req_1",
            "claude-opus-5-5",
            usage,
        ),
        claude_line(
            "2026-10-16T08:00:00Z",
            "msg_5",
            "req_5",
            "claude-opus-5-5",
            r#"{"input_tokens":1,"output_tokens":1}"#,
        ),
    ]
    .join("\n");
    fs.add_file(
        dir.join(r"projects\C--work-app\22222222.jsonl"),
        resumed,
        ts("2026-10-16T08:00:00Z"),
    );
    fs.add_file(
        dir.join(r"projects\C--work-app\22222222\subagents\agent-a.jsonl"),
        claude_line(
            "2026-10-16T08:01:00Z",
            "msg_6",
            "req_6",
            "claude-sonnet-5-5",
            r#"{"input_tokens":2,"output_tokens":2}"#,
        ),
        ts("2026-10-16T08:01:00Z"),
    );
    // Older than the period, and not a log.
    fs.add_file(
        dir.join(r"projects\C--old\33333333.jsonl"),
        claude_line(
            "2026-01-01T00:00:00Z",
            "msg_7",
            "req_7",
            "claude-opus-5-5",
            usage,
        ),
        ts("2026-01-01T00:00:00Z"),
    );
    fs.add_file(
        dir.join(r"projects\notes.txt"),
        "x",
        ts("2026-10-16T00:00:00Z"),
    );

    let log = read_claude(&fs, dir, ts("2026-10-01T00:00:00Z"));
    assert_eq!(log.files, 3);
    assert_eq!(log.requests.len(), 4, "{:?}", log.requests);
    let first = log
        .requests
        .iter()
        .find(|r| model_of(&log, r) == "claude-opus-5-5" && r.output == 5)
        .expect("msg_1");
    assert_eq!(first.at, ts("2026-10-15T10:00:01Z"));
    assert_eq!(
        (
            first.input,
            first.cache_read,
            first.cache_write_5m,
            first.cache_write_1h
        ),
        (10, 2000, 100, 200)
    );
    let haiku = log
        .requests
        .iter()
        .find(|r| model_of(&log, r) == "claude-haiku-5-5")
        .expect("msg_4");
    assert_eq!((haiku.cache_write_5m, haiku.cache_write_1h), (40, 0));
    assert!(
        log.requests
            .iter()
            .any(|r| model_of(&log, r) == "claude-sonnet-5-5")
    );
    assert!(!log.models.iter().any(|m| m == "<synthetic>"));
}

#[test]
fn a_folder_without_projects_has_no_files() {
    let fs = FakeFs::new();
    let log = read_claude(&fs, Path::new(r"C:\nowhere"), ts("2026-10-01T00:00:00Z"));
    assert_eq!(log, TokenLog::default());
}

fn token_count(at: &str, total: (u64, u64, u64), last: Option<(u64, u64, u64)>) -> String {
    let usage = |(i, c, o): (u64, u64, u64)| {
        format!(
            r#"{{"input_tokens":{i},"cached_input_tokens":{c},"output_tokens":{o},"reasoning_output_tokens":0,"total_tokens":{}}}"#,
            i + o
        )
    };
    let last = last.map_or_else(|| "null".to_owned(), usage);
    format!(
        r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{last},"model_context_window":272000}},"rate_limits":null}}}}"#,
        usage(total)
    )
}

fn turn_context(model: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-10-15T09:00:00Z","type":"turn_context","payload":{{"cwd":"C:\\invented","model":"{model}","effort":"medium"}}}}"#
    )
}

#[test]
fn codex_rollouts_give_one_request_per_new_total() {
    let home = Path::new(r"C:\Users\test\.codex");
    let fs = FakeFs::new();
    let rollout = [
        r#"{"timestamp":"2026-10-15T09:00:00Z","type":"session_meta","payload":{"id":"x"}}"#
            .to_owned(),
        // Before any turn context: the model is unknown.
        token_count("2026-10-15T09:00:30Z", (100, 0, 10), Some((100, 0, 10))),
        turn_context("gpt-5.5"),
        token_count("2026-10-15T09:01:00Z", (1100, 800, 60), Some((1000, 800, 50))),
        // The same totals again (a rate-limit update): not a request.
        token_count("2026-10-15T09:01:05Z", (1100, 800, 60), Some((1000, 800, 50))),
        // No last usage: the difference of the totals.
        token_count("2026-10-15T09:02:00Z", (1600, 1200, 70), None),
        r#"{"timestamp":"2026-10-15T09:03:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{}}}"#
            .to_owned(),
    ]
    .join("\n");
    fs.add_file(
        home.join(r"sessions\2026\10\15\rollout-2026-10-15T09-00-00-aaaa.jsonl"),
        rollout,
        ts("2026-10-15T09:03:00Z"),
    );
    let archived = [
        turn_context("gpt-5.1-codex-max"),
        token_count("2026-10-02T10:00:00Z", (50, 20, 5), Some((50, 20, 5))),
    ]
    .join("\n");
    fs.add_file(
        home.join(r"archived_sessions\rollout-2026-10-02T10-00-00-bbbb.jsonl.zst"),
        ruzstd::encoding::compress_to_vec(
            archived.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        ),
        ts("2026-10-02T10:00:00Z"),
    );
    fs.add_file(
        home.join(r"sessions\2026\10\15\other.jsonl"),
        token_count("2026-10-15T09:00:00Z", (9, 9, 9), None),
        ts("2026-10-15T09:00:00Z"),
    );

    let log = read_codex(&fs, home, ts("2026-10-01T00:00:00Z"));
    assert_eq!(log.files, 2);
    let rows: Vec<(&str, u64, u64, u64)> = log
        .requests
        .iter()
        .map(|r| (model_of(&log, r), r.input, r.cache_read, r.output))
        .collect();
    assert_eq!(
        rows,
        [
            (UNKNOWN_MODEL, 100, 0, 10),
            ("gpt-5.5", 200, 800, 50),
            ("gpt-5.5", 100, 400, 10),
            ("gpt-5.1-codex-max", 30, 20, 5),
        ]
    );
    assert!(log.requests.iter().all(|r| r.cache_write_5m == 0));
}
