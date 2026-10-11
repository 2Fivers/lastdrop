//! JSON shapes shared by the Claude sources, all tolerant: a field that is missing, null or of an
//! unexpected type becomes `None` instead of failing the document (ARCHITECTURE "Parsers tolerate
//! unknown and missing fields; a missing value is `None`, never 0").
//!
//! The usage endpoint's body and Claude Code's cached copy of it share [`UsageBody`]; Claude
//! Desktop's history samples use [`WindowValue`] for their bare percentages.

use jiff::Timestamp;
use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer, Error as _};
use serde_json::Value;

use super::{ClaudeWindow, WindowReading};

/// A value whose shape may drift: anything that does not deserialize as `T` is `None`.
///
/// Buffers the value as a [`Value`] first, so use it only around small subtrees, never around
/// secrets (the buffer is not wiped).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Lenient<T>(pub(crate) Option<T>);

impl<T> Default for Lenient<T> {
    fn default() -> Lenient<T> {
        Lenient(None)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Lenient<T>, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Lenient(T::deserialize(value).ok()))
    }
}

/// An instant as the Claude sources write it: RFC 3339 text with an offset (`resets_at`), or
/// epoch seconds or milliseconds (`t`, `fetchedAtMs`, `expiresAt`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Instant(pub(crate) Timestamp);

impl<'de> Deserialize<'de> for Instant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Instant, D::Error> {
        let value = Value::deserialize(deserializer)?;
        instant_of(&value)
            .map(Instant)
            .ok_or_else(|| D::Error::custom("not an instant"))
    }
}

/// The instant in `value`: RFC 3339 text, or a positive epoch number (see [`epoch`]).
pub(crate) fn instant_of(value: &Value) -> Option<Timestamp> {
    match value {
        Value::String(text) => text.trim().parse::<Timestamp>().ok(),
        Value::Number(number) => epoch(number.as_f64()?),
        _ => None,
    }
}

/// Epoch seconds or milliseconds: values from 10^11 on are milliseconds (10^11 seconds is the
/// year 5138). `None` for zero, negative or out-of-range values (Claude Code writes
/// `expiresAt: 0` when it keeps no token in the file).
pub(crate) fn epoch(number: f64) -> Option<Timestamp> {
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    let millis = if number >= 1e11 {
        number
    } else {
        number * 1000.0
    };
    if millis >= i64::MAX as f64 {
        return None;
    }
    Timestamp::from_millisecond(millis.round() as i64).ok()
}

/// A used share from a percentage (`utilization`, `percent`, history values): `p / 100`,
/// clamped at 0 (R§3.1), may exceed 1. `None` when missing or not a finite number.
pub(crate) fn share(percent: Option<f64>) -> Option<f64> {
    let percent = percent?;
    percent.is_finite().then(|| (percent / 100.0).max(0.0))
}

/// One window's value: `{"utilization": 42.0, "resets_at": "…"}` (the usage endpoint) or a bare
/// percentage (Claude Desktop's history samples). An object may say `percent` instead of
/// `utilization` (the `limits[]` entries).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WindowValue {
    /// Percent used, when given as a number.
    pub(crate) percent: Option<f64>,
    /// When the window resets, when given and readable.
    pub(crate) resets_at: Option<Timestamp>,
}

impl WindowValue {
    fn of(value: &Value) -> Option<WindowValue> {
        match value {
            Value::Number(number) => Some(WindowValue {
                percent: number.as_f64(),
                resets_at: None,
            }),
            Value::Object(map) => Some(WindowValue {
                percent: map
                    .get("utilization")
                    .and_then(Value::as_f64)
                    .or_else(|| map.get("percent").and_then(Value::as_f64)),
                resets_at: map.get("resets_at").and_then(instant_of),
            }),
            _ => None,
        }
    }

    /// The reading of `window`, or `None` when the percentage is missing (never 0).
    pub(crate) fn reading(self, window: ClaudeWindow) -> Option<WindowReading> {
        Some(WindowReading {
            window,
            used: share(self.percent)?,
            resets_at: self.resets_at,
        })
    }
}

impl<'de> Deserialize<'de> for WindowValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<WindowValue, D::Error> {
        let value = Value::deserialize(deserializer)?;
        WindowValue::of(&value).ok_or_else(|| D::Error::custom("not a usage window"))
    }
}

/// The body of `GET /api/oauth/usage`, which Claude Code also caches in `.claude.json`. Only the
/// windows of R§2 are read; every other key (`extra_usage`, `spend`, `iguana_necktie`,
/// `seven_day_omelette`, `cedar_ember`, codenames that come and go) is ignored.
///
/// Current shape: `five_hour` and `seven_day` objects, and `limits[]` with
/// `{kind: "session" | "weekly_all" | "weekly_scoped", percent, resets_at, scope: {model:
/// {display_name}}}` carrying the per-model weekly windows (`Fable`). Older bodies had the
/// per-model windows as `seven_day_opus` / `seven_day_sonnet` objects and no `limits`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct UsageBody {
    five_hour: Lenient<WindowValue>,
    seven_day: Lenient<WindowValue>,
    seven_day_opus: Lenient<WindowValue>,
    seven_day_sonnet: Lenient<WindowValue>,
    limits: Lenient<Vec<Value>>,
}

/// A `limits[]` entry, read field by field.
struct Limit<'a> {
    kind: Option<&'a str>,
    value: WindowValue,
    model: Option<&'a str>,
}

impl<'a> Limit<'a> {
    fn of(value: &'a Value) -> Option<Limit<'a>> {
        let map = value.as_object()?;
        Some(Limit {
            kind: map.get("kind").and_then(Value::as_str),
            value: WindowValue::of(value)?,
            model: map
                .get("scope")
                .and_then(|scope| scope.get("model"))
                .and_then(|model| model.get("display_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty()),
        })
    }
}

impl UsageBody {
    /// The R§2 windows in the service's order: 5-hour, weekly, then the per-model weekly windows
    /// (from `limits[]`, then the legacy `seven_day_opus` / `seven_day_sonnet` for models not
    /// listed there). The top-level `five_hour` / `seven_day` objects win over the `limits[]`
    /// `session` / `weekly_all` entries, whose percentages are rounded.
    pub(crate) fn windows(&self) -> Vec<WindowReading> {
        let limits: Vec<Limit<'_>> = self
            .limits
            .0
            .iter()
            .flatten()
            .filter_map(Limit::of)
            .collect();
        let limit_of_kind = |kind: &str| limits.iter().find(|l| l.kind == Some(kind));

        let mut out = Windows::default();
        let five = self
            .five_hour
            .0
            .and_then(|v| v.reading(ClaudeWindow::FiveHour))
            .or_else(|| {
                limit_of_kind("session")?
                    .value
                    .reading(ClaudeWindow::FiveHour)
            });
        out.push(five);
        let weekly = self
            .seven_day
            .0
            .and_then(|v| v.reading(ClaudeWindow::Weekly))
            .or_else(|| {
                limit_of_kind("weekly_all")?
                    .value
                    .reading(ClaudeWindow::Weekly)
            });
        out.push(weekly);
        for limit in limits.iter().filter(|l| l.kind == Some("weekly_scoped")) {
            // A scope without a model (a surface) or the generic "All models" is not a
            // per-model window.
            if let Some(model) = limit
                .model
                .filter(|m| !m.eq_ignore_ascii_case("all models"))
            {
                out.push_model(model, Some(limit.value));
            }
        }
        out.push_model("Opus", self.seven_day_opus.0);
        out.push_model("Sonnet", self.seven_day_sonnet.0);
        out.0
    }
}

/// Windows being collected, without duplicates.
#[derive(Default)]
pub(crate) struct Windows(pub(crate) Vec<WindowReading>);

impl Windows {
    /// Adds a reading unless that window is already there.
    pub(crate) fn push(&mut self, reading: Option<WindowReading>) {
        if let Some(reading) = reading
            && !self.0.iter().any(|r| r.window == reading.window)
        {
            self.0.push(reading);
        }
    }

    /// Adds the weekly window of `model` unless a window of that model (any case) is there.
    pub(crate) fn push_model(&mut self, model: &str, value: Option<WindowValue>) {
        let known = self.0.iter().any(|r| match &r.window {
            ClaudeWindow::WeeklyModel(m) => m.eq_ignore_ascii_case(model),
            _ => false,
        });
        if !known {
            self.push(value.and_then(|v| v.reading(ClaudeWindow::WeeklyModel(model.to_owned()))));
        }
    }
}

/// A secret-free parse error detail: position only, never serde's message (which can quote the
/// offending value).
pub(crate) fn json_detail(name: &str, error: &serde_json::Error) -> String {
    format!(
        "{name}: invalid JSON at line {}, column {}",
        error.line(),
        error.column()
    )
}
