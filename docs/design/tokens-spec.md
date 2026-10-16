# Usage history: tokens and cost

ROADMAP M3 "Charts": the part charts-spec §9 left. The usage history window gains a section with
the tokens Claude Code and Codex used per day and per model on this PC, and what they would have
cost at the API's list prices.

References: charts-spec (C§), ARCHITECTURE "Providers" (the rules every reader of another tool's
files follows), accounts-spec (a source is a config folder).

## 1. Where the numbers come from

Only the tools' own session logs on this PC, read-only, the usage fields only. No prompt, answer,
file name or project path is kept or logged; nothing leaves the machine.

- **Claude Code** (a Claude source's config folder: `CLAUDE_CONFIG_DIR`, else `~\.claude`, or the
  account's own folder): every `*.jsonl` under `<config_dir>\projects`, at most eight folders deep
  (a project folder, a session's folder, its `subagents`, their `workflows\wf_…` folders). A line counts when it is an
  `assistant` message with `message.usage` and `message.model`: its `timestamp`, its model, and the
  token counts `input_tokens`, `output_tokens`, `cache_read_input_tokens` and
  `cache_creation_input_tokens`, split into 5-minute and 1-hour writes by
  `cache_creation.ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens` when present (else all
  5-minute). Claude Code writes one line per content block of a message, and a resumed session
  copies earlier messages into its new file, so lines are one request by `message.id` and
  `requestId` across all files; of such a group the line with the most tokens wins, at the time of the first. Model
  `<synthetic>` (Claude Code's own placeholder messages) and lines whose counts are all zero do
  not count.
- **Codex** (a Codex source's `CODEX_HOME`, else `~\.codex`): the rollouts in
  `sessions\YYYY\MM\DD\rollout-*.jsonl` and `archived_sessions\rollout-*.jsonl`, and their
  compressed form `.jsonl.zst`. The model of a request is the last `turn_context` line's
  `payload.model` before it. A request is an `event_msg` `token_count` whose `info.total_token_usage`
  differs from the one before it in the file (Codex repeats the event when only the rate limits
  changed); its counts are `info.last_token_usage` (else the difference of the totals):
  `input_tokens` (cached ones included), `cached_input_tokens`, `output_tokens` (reasoning
  included).
- **When:** files changed since a day before the period's start, read when the window first
  shows a source for that period; a longer period reads again, a shorter one does not. A line that does
  not parse is skipped. Claude Code deletes its own transcripts after 30 days unless the user set
  `cleanupPeriodDays`, so a longer period can be shorter in fact; the section says what it found.

The readers are `lastdrop_providers::tokens` (through the `Fs` trait, so tests run on invented
fixtures); the model of the section is `lastdrop_core::tokens`, pure.

## 2. Prices

`crates/core/prices.json` is compiled in: per model, USD per million tokens for `input`,
`output`, `cache_write_5m`, `cache_write_1h` and `cache_read`, a display `name`, and optionally a
`long` tier (`above`: a request whose prompt is more tokens than this pays the tier's prices).
The file names the day it was checked against the vendors' price pages (`checked`). A release
updates it; between releases the user can add or change models in
`%APPDATA%\Lastdrop\prices.json` (same format; an entry there replaces the bundled one with the
same `id`). A broken override file is ignored and logged.

- **Matching a model:** its id equals an entry's `id`, or is the `id` followed by a date
  (`-20250929`, `-2025-09-29`, `@20250929`). A leading `anthropic.` is ignored. Nothing else
  matches: an unknown model has no price rather than a wrong one.
- **Missing prices in an entry:** a missing cache price is the input price. An entry without
  `input` or `output` is skipped.
- **A request's prompt** for the `long` tier: Claude's input + cache reads + cache writes; Codex's
  input (which includes the cached part).
- **Cost of a request:** Claude: input × input + 5-minute writes × `cache_write_5m` + 1-hour
  writes × `cache_write_1h` + cache reads × `cache_read` + output × output. Codex: (input −
  cached) × input + cached × `cache_read` + output × output.
- **Honesty:** the cost is what the requests would cost at the API's list prices. Claude and
  ChatGPT plans do not pay per token; the section says so. A model without a price shows `–`, and
  the total then says it leaves those out.

## 3. The section

Shown for Claude and Codex sources only, after **Active hours**. Title **Tokens and cost**.

- While reading: `Reading session logs…`.
- No log files at all: `No session logs from {client} on this PC for this account.`
- None in the period: `No tokens were used in this period.`
- **Tokens per day:** a column per local day of the period, all tokens of the day (input, output,
  cache writes and reads), drawn like C§2's use per day (today in the accent colour). Its scale
  label is the round top (C§3 "Scales") as a count. Under it:
  `{tokens} tokens · ≈ {cost} at API prices · highest {max} on {date}`, or without a price for any
  model `{tokens} tokens · highest {max} on {date}`.
- **Per model:** one row per model used in the period, most tokens first: the model's name (the
  price table's, else the id as the log gives it) and, under it, `In {input} · out {output} ·
  cache {cache}` (cache = writes and reads); on the right its tokens and its cost
  (`≈ 12.40 USD`, or `–` without a price). Then a **Total** row.
- **Note:** `From {client}'s session logs on this PC. Cost at API list prices, checked {date}; a
  plan does not pay per token.`
- **Counts** are compact: `950`, `12K`, `4.2M`, `5M`, `1.3B` (one decimal below 10 unless it is 0, none from 10, in the
  regional decimal separator; the suffixes are strings: Swedish `12 tn`, `4,2 mn`, `1,3 md`).
  **Money** is two decimals below 100 and whole from 100, grouped, with the code after it:
  `12.40 USD`, `1,240 USD`; Swedish `12,40 USD`.

## 4. Threads and memory

The tray process gains nothing. In the dialog process the logs are read on a `tokens` thread
(they can be gigabytes: one heavy month of Claude Code measured 4.6 GB in 1,400 files), which
reads up to four files at once; the window shows `Reading…` meanwhile and the result reaches the
main thread with `slint::invoke_from_event_loop`. A result for a source the window has moved away
from is kept for when it comes back. Each line is filtered by a byte search (`memchr::memmem`:
`"type":"assistant"` and `"usage":`, or `token_count` / `turn_context`) before it is parsed, and
parsed into a struct that keeps only the fields above; copies are told apart by a 64-bit hash of
`message.id` and `requestId`. One request is about 60 bytes; 100,000 requests are 6 MB, held while
the window is open. Measured on 2026-10-16 (release build, warm file cache): 4.6 GB in about
5 seconds.

## 5. Accessibility

The column chart is an image labelled `Tokens per day, {span}: {tokens} tokens in all, highest
{max} on {date}.` The model rows are plain text.

## 6. Texts

`history.tokens`, `history.tokens.reading`, `history.tokens.none`, `history.tokens.empty`,
`history.tokens.summary`, `history.tokens.summary.nocost`, `history.tokens.split`,
`history.tokens.total`, `history.tokens.note`, `history.tokens.count.k`, `.m`, `.b`,
`a11y.history.tokens`.
