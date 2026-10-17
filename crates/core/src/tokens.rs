//! Tokens and cost per day and model (docs/design/tokens-spec.md): the requests Claude Code and
//! Codex logged on this PC, priced with the API's list prices, as the usage history window's
//! section. Pure: the logs are read by `lastdrop_providers::tokens`, the override file by the app.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::Deserialize;

use crate::charts::{DayColumn, Period, Range, nice_max, shown_date, spoken_date};
use crate::grammar::LocaleFormat;
use crate::grammar::numbers::{money, token_count};
use crate::grammar::strings::{Msg, Strings, text};

/// The bundled price table (tokens-spec §2).
pub const BUNDLED_PRICES: &str = include_str!("../prices.json");

/// One request a tool logged: when, which model (an index into [`TokenLog::models`]) and its
/// tokens. Codex's cached input is `cache_read` and its `input` is the uncached part, so one
/// formula prices both tools (tokens-spec §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// When it was made.
    pub at: Timestamp,
    /// Its model: an index into [`TokenLog::models`].
    pub model: u16,
    /// Input tokens read neither from nor into the cache.
    pub input: u64,
    /// Output tokens (reasoning included).
    pub output: u64,
    /// Input tokens read from the cache.
    pub cache_read: u64,
    /// Input tokens written to the 5-minute cache.
    pub cache_write_5m: u64,
    /// Input tokens written to the 1-hour cache.
    pub cache_write_1h: u64,
}

impl Request {
    /// Every token of the request.
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache())
    }

    /// Cache writes and reads.
    pub fn cache(&self) -> u64 {
        self.cache_read
            .saturating_add(self.cache_write_5m)
            .saturating_add(self.cache_write_1h)
    }

    /// The prompt's length for a `long` price tier: everything read in.
    pub fn prompt(&self) -> u64 {
        self.input.saturating_add(self.cache())
    }
}

/// What a tool logged for one account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenLog {
    /// The model ids as the logs name them.
    pub models: Vec<String>,
    /// The requests, in no particular order.
    pub requests: Vec<Request>,
    /// Log files found (whether or not they held a request): none means the tool never ran in
    /// this account on this PC (`history.tokens.none`).
    pub files: usize,
}

impl TokenLog {
    /// The index of `model`, added when new; `None` beyond `u16` models.
    pub fn model_index(&mut self, model: &str) -> Option<u16> {
        if let Some(i) = self.models.iter().position(|m| m == model) {
            return u16::try_from(i).ok();
        }
        let i = u16::try_from(self.models.len()).ok()?;
        self.models.push(model.to_owned());
        Some(i)
    }
}

/// Prices per million tokens of one tier.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Rates {
    /// Uncached input.
    pub input: f64,
    /// Output.
    pub output: f64,
    /// 5-minute cache writes; the input price when absent.
    #[serde(default)]
    pub cache_write_5m: Option<f64>,
    /// 1-hour cache writes; the input price when absent.
    #[serde(default)]
    pub cache_write_1h: Option<f64>,
    /// Cache reads; the input price when absent.
    #[serde(default)]
    pub cache_read: Option<f64>,
}

impl Rates {
    /// The cost of `r` at these rates.
    fn cost(&self, r: &Request) -> f64 {
        let per = |tokens: u64, price: f64| tokens as f64 * price / 1e6;
        per(r.input, self.input)
            + per(r.output, self.output)
            + per(r.cache_read, self.cache_read.unwrap_or(self.input))
            + per(r.cache_write_5m, self.cache_write_5m.unwrap_or(self.input))
            + per(r.cache_write_1h, self.cache_write_1h.unwrap_or(self.input))
    }

    /// Every price is a finite number, none negative.
    fn valid(&self) -> bool {
        [
            Some(self.input),
            Some(self.output),
            self.cache_write_5m,
            self.cache_write_1h,
            self.cache_read,
        ]
        .into_iter()
        .flatten()
        .all(|p| p.is_finite() && p >= 0.0)
    }
}

/// A higher tier for long prompts.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct LongRates {
    /// A request whose prompt has more tokens than this pays these rates.
    pub above: u64,
    /// The rates.
    #[serde(flatten)]
    pub rates: Rates,
}

/// One model's prices.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ModelPrice {
    /// The model id without a date (`claude-opus-5-5`, `gpt-5.5`).
    pub id: String,
    /// Its display name (`Opus 5.5`); the id when absent.
    #[serde(default)]
    pub name: Option<String>,
    /// The rates.
    #[serde(flatten)]
    pub rates: Rates,
    /// The long-prompt tier.
    #[serde(default)]
    pub long: Option<LongRates>,
}

impl ModelPrice {
    /// The cost of `r`.
    pub fn cost(&self, r: &Request) -> f64 {
        match &self.long {
            Some(long) if r.prompt() > long.above => long.rates.cost(r),
            _ => self.rates.cost(r),
        }
    }
}

/// A price file as written (tokens-spec §2). Entries that do not parse are skipped one by one.
#[derive(Deserialize)]
struct PriceFile {
    #[serde(default)]
    checked: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    models: Vec<serde_json::Value>,
}

/// The prices in use: the bundled table with the user's file over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceTable {
    /// The day the bundled table was checked (`2026-10-16`).
    pub checked: Option<jiff::civil::Date>,
    /// The currency (`USD`).
    pub currency: String,
    /// The models, by id.
    pub models: BTreeMap<String, ModelPrice>,
}

impl PriceTable {
    /// Parses a price file; `Err` when it is not a price file at all. Entries without `id`,
    /// `input` or `output`, or with a negative price, are left out.
    pub fn parse(json: &str) -> Result<PriceTable, serde_json::Error> {
        let file: PriceFile = serde_json::from_str(json)?;
        let models = file
            .models
            .into_iter()
            .filter_map(|v| serde_json::from_value::<ModelPrice>(v).ok())
            .filter(|m| {
                !m.id.trim().is_empty()
                    && m.rates.valid()
                    && m.long.as_ref().is_none_or(|l| l.rates.valid())
            })
            .map(|m| (m.id.to_ascii_lowercase(), m))
            .collect();
        Ok(PriceTable {
            checked: file.checked.and_then(|d| d.parse().ok()),
            currency: file
                .currency
                .filter(|c| !c.trim().is_empty())
                .unwrap_or_else(|| "USD".to_owned()),
            models,
        })
    }

    /// The bundled table; empty if it does not parse (a test makes sure it does).
    pub fn bundled() -> PriceTable {
        PriceTable::parse(BUNDLED_PRICES).unwrap_or_default()
    }

    /// This table with the entries of `user` over it (same id: the user's wins). The checked
    /// date and the currency stay the bundled table's.
    pub fn with(mut self, user: PriceTable) -> PriceTable {
        self.models.extend(user.models);
        self
    }

    /// The price of the model `id` as a log names it (tokens-spec §2 "Matching a model").
    pub fn price(&self, id: &str) -> Option<&ModelPrice> {
        let id = id.trim().to_ascii_lowercase();
        let id = id.strip_prefix("anthropic.").unwrap_or(&id);
        if let Some(p) = self.models.get(id) {
            return Some(p);
        }
        let (base, _date) = split_date(id)?;
        self.models.get(base)
    }
}

/// `id` split before a trailing date (`-20250929`, `-2025-09-29`, `@20250929`); `None` without
/// one.
fn split_date(id: &str) -> Option<(&str, &str)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let compact = |s: &str| s.len() == 8 && digits(s);
    let dashed = |s: &str| {
        let parts: Vec<&str> = s.split('-').collect();
        matches!(parts.as_slice(), [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2
            && digits(y) && digits(m) && digits(d))
    };
    if let Some(at) = id.rfind('@') {
        let (base, date) = (&id[..at], &id[at + 1..]);
        return (compact(date) || dashed(date)).then_some((base, date));
    }
    if id.len() > 11 {
        let cut = id.len() - 11;
        if id.is_char_boundary(cut) && id.as_bytes()[cut] == b'-' && dashed(&id[cut + 1..]) {
            return Some((&id[..cut], &id[cut + 1..]));
        }
    }
    let dash = id.rfind('-')?;
    let date = &id[dash + 1..];
    compact(date).then_some((&id[..dash], date))
}

/// What [`build`] works from.
pub struct TokensInput<'a> {
    /// What the tool logged.
    pub log: &'a TokenLog,
    /// The prices.
    pub prices: &'a PriceTable,
    /// The tool's name (`Claude Code`, `Codex`).
    pub client: &'a str,
    /// The period.
    pub range: Range,
    /// The clock.
    pub now: Timestamp,
    /// Regional format and time zone.
    pub locale: &'a LocaleFormat,
    /// The language.
    pub strings: &'a dyn Strings,
}

/// The section for one account and period (tokens-spec §3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokensModel {
    /// Why there is nothing to show (`history.tokens.none`, `history.tokens.empty`); the rest is
    /// empty then.
    pub empty: Option<String>,
    /// Tokens per day, oldest first.
    pub columns: Vec<DayColumn>,
    /// The top of the column scale (`20M`).
    pub top: String,
    /// The line under the columns.
    pub summary: String,
    /// The columns, spoken.
    pub a11y: String,
    /// One row per model, most tokens first.
    pub rows: Vec<ModelRow>,
    /// The total row.
    pub total: Option<ModelRow>,
    /// Where the numbers come from and what the cost means.
    pub note: String,
}

/// One row of the per-model list.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRow {
    /// The model's name.
    pub name: String,
    /// `In 1.2M · out 340K · cache 48M`.
    pub split: String,
    /// Its tokens (`52M`).
    pub tokens: String,
    /// `≈ 12.40 USD`, or `–` without a price.
    pub cost: String,
}

/// Sums of one model or of everything.
#[derive(Clone, Copy, Debug, Default)]
struct Sums {
    input: u64,
    output: u64,
    cache: u64,
    cost: f64,
    /// Some requests had no price.
    unpriced: bool,
}

impl Sums {
    fn add(&mut self, r: &Request, cost: Option<f64>) {
        self.input = self.input.saturating_add(r.input);
        self.output = self.output.saturating_add(r.output);
        self.cache = self.cache.saturating_add(r.cache());
        match cost {
            Some(c) => self.cost += c,
            None => self.unpriced = true,
        }
    }

    fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache)
    }
}

/// The text `history.tokens.none` for `client`, shown when an account has no logs.
pub fn none_text(client: &str, strings: &dyn Strings) -> String {
    text(strings, Msg::HistoryTokensNone, &[("client", client)])
}

/// The section of `input.log` over `input.range` (tokens-spec §3).
pub fn build(input: &TokensInput<'_>) -> TokensModel {
    let strings = input.strings;
    let locale = input.locale;
    if input.log.files == 0 {
        return TokensModel {
            empty: Some(none_text(input.client, strings)),
            ..TokensModel::default()
        };
    }
    let tz = &locale.tz;
    let period = Period::of(input.range, input.now, tz);
    let prices: Vec<Option<&ModelPrice>> = input
        .log
        .models
        .iter()
        .map(|m| input.prices.price(m))
        .collect();
    let mut days = vec![0u64; period.days];
    let mut per_model: BTreeMap<u16, Sums> = BTreeMap::new();
    let mut all = Sums::default();
    for r in &input.log.requests {
        if r.at < period.from || r.at >= period.to || r.total() == 0 {
            continue;
        }
        let Some(day) = day_index(r.at, &period, locale) else {
            continue;
        };
        if let Some(slot) = days.get_mut(day) {
            *slot = slot.saturating_add(r.total());
        }
        let cost = prices
            .get(usize::from(r.model))
            .copied()
            .flatten()
            .map(|p| p.cost(r));
        per_model.entry(r.model).or_default().add(r, cost);
        all.add(r, cost);
    }
    if all.total() == 0 {
        return TokensModel {
            empty: Some(strings.template(Msg::HistoryTokensEmpty).to_owned()),
            ..TokensModel::default()
        };
    }

    let count = |n: u64| token_count(n, locale, strings);
    let currency = input.prices.currency.as_str();
    let cost_text = |sums: &Sums, any_price: bool| {
        if any_price {
            text(
                strings,
                Msg::HistoryTokensCost,
                &[("cost", &money(sums.cost, currency, locale))],
            )
        } else {
            "–".to_owned()
        }
    };

    // Tokens per day.
    let (best_day, best) = days
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .unwrap_or((0, 0));
    let top = nice_max((best as f64).max(1.0));
    let last = period.days.saturating_sub(1);
    let columns = days
        .iter()
        .enumerate()
        .map(|(i, &n)| DayColumn {
            height: (n as f64 / top).clamp(0.0, 1.0) as f32,
            today: i == last,
        })
        .collect();
    let best_start = period.day_start(best_day, tz).unwrap_or(period.from);
    let any_price = per_model
        .keys()
        .any(|&i| prices.get(usize::from(i)).copied().flatten().is_some());
    let summary = if any_price {
        text(
            strings,
            Msg::HistoryTokensSummary,
            &[
                ("tokens", &count(all.total())),
                ("cost", &money(all.cost, currency, locale)),
                ("max", &count(best)),
                ("date", &shown_date(best_start, locale)),
            ],
        )
    } else {
        text(
            strings,
            Msg::HistoryTokensSummaryNoCost,
            &[
                ("tokens", &count(all.total())),
                ("max", &count(best)),
                ("date", &shown_date(best_start, locale)),
            ],
        )
    };
    let span = text(
        strings,
        Msg::A11yHistorySpan,
        &[("n", &period.days.to_string())],
    );
    let a11y = text(
        strings,
        Msg::A11yHistoryTokens,
        &[
            ("span", &span),
            ("tokens", &count(all.total())),
            ("max", &count(best)),
            ("date", &spoken_date(best_start, locale)),
        ],
    );

    // Per model, most tokens first (ties: name).
    let row = |name: String, sums: &Sums, priced: bool| ModelRow {
        name,
        split: text(
            strings,
            Msg::HistoryTokensSplit,
            &[
                ("input", &count(sums.input)),
                ("output", &count(sums.output)),
                ("cache", &count(sums.cache)),
            ],
        ),
        tokens: count(sums.total()),
        cost: cost_text(sums, priced),
    };
    let mut models: Vec<(String, Sums, bool)> = per_model
        .iter()
        .map(|(&i, sums)| {
            let id = input
                .log
                .models
                .get(usize::from(i))
                .map_or("", String::as_str);
            let price = prices.get(usize::from(i)).copied().flatten();
            let name = price
                .and_then(|p| p.name.clone())
                .unwrap_or_else(|| id.to_owned());
            (name, *sums, price.is_some())
        })
        .collect();
    // The same name from two ids (a dated and an undated id) is one row.
    models.sort_by(|a, b| a.0.cmp(&b.0));
    let mut merged: Vec<(String, Sums, bool)> = Vec::new();
    for (name, sums, priced) in models {
        match merged.last_mut() {
            Some(last) if last.0 == name => {
                last.1.input = last.1.input.saturating_add(sums.input);
                last.1.output = last.1.output.saturating_add(sums.output);
                last.1.cache = last.1.cache.saturating_add(sums.cache);
                last.1.cost += sums.cost;
                last.1.unpriced |= sums.unpriced;
                last.2 &= priced;
            }
            _ => merged.push((name, sums, priced)),
        }
    }
    merged.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(&b.0)));
    let rows: Vec<ModelRow> = merged
        .iter()
        .map(|(name, sums, priced)| row(name.clone(), sums, *priced))
        .collect();
    let total_label = if all.unpriced && any_price {
        Msg::HistoryTokensTotalPartial
    } else {
        Msg::HistoryTokensTotal
    };
    let total = row(strings.template(total_label).to_owned(), &all, any_price);

    let checked = input
        .prices
        .checked
        .and_then(|d| d.to_zoned(tz.clone()).ok())
        .map(|z| crate::grammar::time::day_month(z.timestamp(), locale))
        .unwrap_or_else(|| "–".to_owned());
    let note = text(
        strings,
        Msg::HistoryTokensNote,
        &[("client", input.client), ("date", &checked)],
    );

    TokensModel {
        empty: None,
        columns,
        top: count(top as u64),
        summary,
        a11y,
        rows,
        total: Some(total),
        note,
    }
}

/// The local day of `at` in `period`, 0 for its first.
fn day_index(at: Timestamp, period: &Period, locale: &LocaleFormat) -> Option<usize> {
    let date = at.to_zoned(locale.tz.clone()).date();
    let days = period.first_day.until(date).ok()?.get_days();
    usize::try_from(days).ok().filter(|&d| d < period.days)
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::*;
    use crate::grammar::{English, Swedish};

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    fn request(at: &str, model: u16, input: u64, output: u64) -> Request {
        Request {
            at: ts(at),
            model,
            input,
            output,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
        }
    }

    #[test]
    fn the_bundled_table_parses_and_prices_every_entry() {
        let table = PriceTable::parse(BUNDLED_PRICES).expect("bundled prices");
        assert!(table.models.len() >= 30, "{}", table.models.len());
        assert_eq!(table.currency, "USD");
        assert!(table.checked.is_some());
        let raw: serde_json::Value = serde_json::from_str(BUNDLED_PRICES).expect("json");
        let listed = raw["models"].as_array().map_or(0, Vec::len);
        assert_eq!(table.models.len(), listed, "every bundled entry is valid");
    }

    #[test]
    fn models_match_by_id_or_id_and_date_only() {
        let table = PriceTable::bundled();
        let name = |id: &str| table.price(id).and_then(|p| p.name.clone());
        assert_eq!(name("claude-opus-5-5").as_deref(), Some("Opus 5.5"));
        assert_eq!(
            name("claude-sonnet-4-5-20250929").as_deref(),
            Some("Sonnet 4.5")
        );
        assert_eq!(name("claude-opus-4-20250514").as_deref(), Some("Opus 4"));
        assert_eq!(
            name("claude-opus-4-1-20250805").as_deref(),
            Some("Opus 4.1")
        );
        assert_eq!(
            name("anthropic.claude-haiku-4-5@20251001").as_deref(),
            Some("Haiku 4.5")
        );
        assert_eq!(name("gpt-5-2025-08-07").as_deref(), Some("GPT-5"));
        assert_eq!(
            name("GPT-5.1-Codex-Max").as_deref(),
            Some("GPT-5.1-Codex-Max")
        );
        // Never a neighbour's price.
        assert_eq!(name("gpt-5.7"), None);
        assert_eq!(name("claude-opus-4-9"), None);
        assert_eq!(name("claude-sonnet-4-7-20260101"), None);
        assert_eq!(name("<synthetic>"), None);
    }

    #[test]
    fn costs_follow_the_tiers() {
        let table = PriceTable::bundled();
        let opus = table.price("claude-opus-5-5").expect("opus");
        let r = Request {
            at: ts("2026-10-10T10:00:00Z"),
            model: 0,
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
        };
        // 4 + 20 + 0.2 + 5 + 8
        assert!((opus.cost(&r) - 37.2).abs() < 1e-9, "{}", opus.cost(&r));

        let haiku = table.price("claude-haiku-5-5").expect("haiku");
        let short = Request {
            input: 100_000,
            output: 0,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            ..r
        };
        assert!((haiku.cost(&short) - 0.01).abs() < 1e-12);
        let long = Request {
            input: 100_001,
            ..short
        };
        assert!((haiku.cost(&long) - 100_001.0 * 0.5 / 1e6).abs() < 1e-12);

        // OpenAI: no cache writes, cached input at the cache price.
        let gpt = table.price("gpt-5.5").expect("gpt");
        let codex = Request {
            input: 200_000,
            cache_read: 72_000,
            output: 10_000,
            cache_write_5m: 0,
            cache_write_1h: 0,
            ..r
        };
        let expected = 0.2 * 5.0 + 0.072 * 0.5 + 0.01 * 30.0;
        assert!((gpt.cost(&codex) - expected).abs() < 1e-9);
        let over = Request {
            cache_read: 72_001,
            ..codex
        };
        let expected = 0.2 * 10.0 + 0.072_001 * 1.0 + 0.01 * 45.0;
        assert!((gpt.cost(&over) - expected).abs() < 1e-9);
    }

    #[test]
    fn the_users_file_replaces_entries_and_bad_ones_are_skipped() {
        let user = PriceTable::parse(
            r#"{"models": [
                {"id": "claude-opus-5-5", "name": "Mine", "input": 1, "output": 2},
                {"id": "new-model", "input": 1, "output": 1},
                {"id": "broken", "input": -1, "output": 1},
                {"id": "no-output", "input": 1},
                "nonsense"
            ]}"#,
        )
        .expect("user prices");
        assert_eq!(user.models.len(), 2);
        let table = PriceTable::bundled().with(user);
        assert_eq!(
            table
                .price("claude-opus-5-5")
                .and_then(|p| p.name.as_deref()),
            Some("Mine")
        );
        assert!(table.price("new-model").is_some());
        assert_eq!(table.checked, PriceTable::bundled().checked);
        assert!(PriceTable::parse("[1, 2]").is_err());
    }

    fn input<'a>(
        log: &'a TokenLog,
        prices: &'a PriceTable,
        locale: &'a LocaleFormat,
        strings: &'a dyn Strings,
    ) -> TokensInput<'a> {
        TokensInput {
            log,
            prices,
            client: "Claude Code",
            range: Range::Week,
            now: ts("2026-10-16T12:00:00Z"),
            locale,
            strings,
        }
    }

    #[test]
    fn the_section_sums_days_and_models() {
        let locale = LocaleFormat::en_gb(TimeZone::UTC);
        let prices = PriceTable::bundled();
        let log = TokenLog {
            models: vec![
                "claude-opus-5-5".to_owned(),
                "claude-opus-5-5-20261001".to_owned(),
                "mystery-model".to_owned(),
            ],
            requests: vec![
                request("2026-10-16T09:00:00Z", 0, 1_000_000, 100_000),
                request("2026-10-16T10:00:00Z", 1, 1_000_000, 100_000),
                request("2026-10-14T10:00:00Z", 2, 50_000, 0),
                // Before the period.
                request("2026-10-01T10:00:00Z", 0, 9_000_000, 0),
            ],
            files: 3,
        };
        let model = build(&input(&log, &prices, &locale, &English));
        assert_eq!(model.empty, None);
        assert_eq!(model.columns.len(), 7);
        assert!(model.columns[6].today);
        assert!(
            (model.columns[6].height - 0.44).abs() < 1e-6,
            "{:?}",
            model.columns
        );
        assert_eq!(model.top, "5M");
        // Opus: 2M in × 4 + 0.2M out × 20 = 12 USD.
        assert_eq!(
            model.summary,
            "2.3M tokens · ≈ 12.00 USD at API prices · highest 2.2M on Fri 16 Oct"
        );
        let names: Vec<&str> = model.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Opus 5.5", "mystery-model"]);
        assert_eq!(model.rows[0].tokens, "2.2M");
        assert_eq!(model.rows[0].split, "In 2M · out 200K · cache 0");
        assert_eq!(model.rows[0].cost, "≈ 12.00 USD");
        assert_eq!(model.rows[1].cost, "–");
        let total = model.total.expect("total");
        assert_eq!(
            total.name,
            "Total (the cost leaves out models without a price)"
        );
        assert_eq!(total.cost, "≈ 12.00 USD");
        assert_eq!(
            model.note,
            "From Claude Code's session logs on this PC. Cost at API list prices, checked 16 Oct; \
             a plan does not pay per token."
        );
        assert!(
            model
                .a11y
                .starts_with("Tokens per day, the last 7 days: 2.3M tokens")
        );
    }

    #[test]
    fn without_logs_or_tokens_it_says_so() {
        let locale = LocaleFormat::en_gb(TimeZone::UTC);
        let prices = PriceTable::bundled();
        let none = TokenLog::default();
        assert_eq!(
            build(&input(&none, &prices, &locale, &English))
                .empty
                .as_deref(),
            Some("No session logs from Claude Code on this PC for this account.")
        );
        let old = TokenLog {
            models: vec!["claude-opus-5-5".to_owned()],
            requests: vec![request("2026-09-01T10:00:00Z", 0, 10, 10)],
            files: 1,
        };
        assert_eq!(
            build(&input(&old, &prices, &locale, &English))
                .empty
                .as_deref(),
            Some("No tokens were used in this period.")
        );
    }

    #[test]
    fn swedish_counts_and_money() {
        let locale = LocaleFormat::sv_se(TimeZone::UTC);
        let prices = PriceTable::bundled();
        let log = TokenLog {
            models: vec!["unknown".to_owned()],
            requests: vec![request("2026-10-16T09:00:00Z", 0, 4_240_000, 0)],
            files: 1,
        };
        let model = build(&input(&log, &prices, &locale, &Swedish));
        assert!(
            model.summary.starts_with("4,2 mn tokens · mest 4,2 mn"),
            "{}",
            model.summary
        );
        assert_eq!(model.total.expect("total").name, "Totalt");
        assert_eq!(money(12.4, "USD", &locale), "12,40 USD");
        assert_eq!(money(1240.4, "USD", &locale), "1\u{202F}240 USD");
    }

    #[test]
    fn compact_counts() {
        let locale = LocaleFormat::en_gb(TimeZone::UTC);
        let c = |n| token_count(n, &locale, &English);
        assert_eq!(c(0), "0");
        assert_eq!(c(950), "950");
        assert_eq!(c(1_000), "1K");
        assert_eq!(c(9_949), "9.9K");
        assert_eq!(c(9_950), "10K");
        assert_eq!(c(12_400), "12K");
        assert_eq!(c(999_499), "999K");
        assert_eq!(c(999_500), "1M");
        assert_eq!(c(4_240_000), "4.2M");
        assert_eq!(c(1_300_000_000), "1.3B");
        assert_eq!(c(2_500_000_000_000), "2,500B");
        assert_eq!(money(0.004, "USD", &locale), "0.00 USD");
        assert_eq!(money(99.999, "USD", &locale), "100 USD");
        assert_eq!(money(f64::NAN, "USD", &locale), "0.00 USD");
    }
}
