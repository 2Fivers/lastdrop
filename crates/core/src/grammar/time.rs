//! Instants and durations (R§5.0), time points (R§5.2), countdowns (R§5.3), ages (R§5.4) and the
//! card/flyout time rule (R§8.2, F§4.2, F§4.4).
//!
//! One rule for every surface (R§5.0): `now` is shown as `floor_min(now)` (as the taskbar clock
//! shows it), every other instant as `round_min(t)` (half up: a reset at `14:59:59.94` shows
//! `15:00`), and every duration shown is `round_min(t) − floor_min(now)`, so the clock, the shown
//! time and the shown duration always add up.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::{SignedDuration, Timestamp};

use super::TextContext;
use super::locale::LocaleFormat;
use super::strings::Strings;

/// `floor_min(t)`: `t` truncated to the minute (R§5.0).
pub fn floor_min(t: Timestamp) -> Timestamp {
    todo!("M1: R§5.0 floor_min")
}

/// `round_min(t)`: `t` rounded to the nearest minute, half up (R§5.0).
pub fn round_min(t: Timestamp) -> Timestamp {
    todo!("M1: R§5.0 round_min")
}

/// A duration as shown: `round_min(t) − floor_min(now)` (R§5.0). Negative when `t` is past.
pub fn shown_duration(now: Timestamp, t: Timestamp) -> SignedDuration {
    todo!("M1: R§5.0")
}

/// An age as shown: `floor_min(now) − round_min(last_success)` (R§5.0).
pub fn shown_age(now: Timestamp, last: Timestamp) -> SignedDuration {
    todo!("M1: R§5.0")
}

/// The readout's `TIME` token (R§5.2), for resets, run-out times and "Back". Clock mode, with
/// `Δ = round_min(t) − floor_min(now)`:
///
/// | Condition                     | Token                                        | Examples          |
/// |-------------------------------|----------------------------------------------|-------------------|
/// | Δ < 12 h                      | short time **without AM/PM**                 | `14:56`, `2:56`   |
/// | same local date, Δ ≥ 12 h     | `Today`                                      | `Today`           |
/// | local date ≤ today + 6        | abbreviated weekday                          | `Mon`, sv `mån`   |
/// | later                         | calendar days between the dates + `d`        | `7d`, `22d`       |
///
/// Relative mode: Δ < 60 min → `Nm` (≥ 1); Δ < 24 h → `Nh` (floor); else `Nd` (floor).
pub fn time_token(t: Timestamp, cx: &TextContext<'_>) -> String {
    todo!("M1: R§5.2")
}

/// The locale's short time of `t` (after `round_min`): `14:56`; with `ampm`, the 12-hour
/// designator where the locale has one (`2:56 PM`), else without (`2:56`) (R§5.2).
pub fn clock_time(t: Timestamp, locale: &LocaleFormat, ampm: bool) -> String {
    todo!("M1: R§5.2 short time")
}

/// Times in the hover card and the flyout (R§8.2, one rule with `round_min`):
/// Δ < 12 h → the full short time with AM/PM (`2:56 PM`); same local date → `Today 18:00`;
/// date ≤ today + 6 → weekday + time (`Mon 09:00`); later → weekday + date, no time
/// (`Sun 1 Nov`).
pub fn card_time(t: Timestamp, cx: &TextContext<'_>) -> String {
    todo!("M1: R§8.2 card time")
}

/// Weekday + date without a time (`Mon 19 Oct`, `Mon Oct 19`): the balance "until" line
/// (R§8.2, F§4.5).
pub fn card_date(t: Timestamp, cx: &TextContext<'_>) -> String {
    todo!("M1: R§8.2 card date")
}

/// A time point in a UI Automation sentence: `at 2:56 PM` within 12 h, else full weekday + time
/// (`Monday 9:00 AM`) (R§11 examples).
pub fn spoken_time(t: Timestamp, cx: &TextContext<'_>) -> String {
    todo!("M1: R§11 spoken time")
}

/// Long durations, hover card and flyout (R§5.0): `< 60 min` → `N min`; `< 24 h` → `H h M min`
/// (`2 h` when M = 0); `< 7 d` → `D d H h` (`3 d` when H = 0); else `D d`. `d` is a shown
/// duration ([`shown_duration`]), whole minutes.
pub fn long_duration(d: SignedDuration, strings: &dyn Strings) -> String {
    todo!("M1: R§5.0 long durations")
}

/// A duration in words for UI Automation (`12 minutes`, `6 hours`, `9 days`): the largest
/// whole unit, with the language's plural forms (R§11).
pub fn spoken_duration(d: SignedDuration, strings: &dyn Strings) -> String {
    todo!("M1: R§11 spoken duration")
}

/// Countdown with `left` or the compact form without it (R§5.3, R§5.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CountdownForm {
    /// `12 min left`, `6 h left`, `3 d left`.
    Left,
    /// `12 min`, `6 h` (compact cell).
    Compact,
}

/// `COUNTDOWN(N)` (R§5.3): `N ≤ 59` → `N min`; `N < 48 × 60` → `H h` (`H = floor(N / 60)`);
/// else `D d` (`D = floor(N / 1440)`). Hours and days floor, so it never promises more than
/// there is. `n_min` is `N` from [`crate::stats::runway_minutes`] (≥ 1).
pub fn countdown(n_min: i64, form: CountdownForm, strings: &dyn Strings) -> String {
    todo!("M1: R§5.3 COUNTDOWN")
}

/// The run-out text of a row (R§5.3):
///
/// ```text
/// runs_out && (CRITICAL || N ≤ 59) → COUNTDOWN(N) + " left"
/// runs_out && otherwise (WARNING)  → "Out " TIME(round_min(t_out))
/// ```
///
/// So WARNING 60 min or more away → `Out 15:20`; WARNING 31–59 min → `45 min left`; CRITICAL →
/// always a countdown (`12 min left`; `6 h left` when critical by level and the forecast runs out
/// later). Balance meters use the same rule with `t_out = now + days_left`.
pub fn runout_text(t_out: Timestamp, critical: bool, cx: &TextContext<'_>) -> String {
    todo!("M1: R§5.3")
}

/// The readout's `AGE` (R§5.4): `floor_min(now) − round_min(last)`; < 60 min → `Nm ago`
/// (N ≥ 1); < 48 h → `Nh ago` (floor); else `Nd ago` (floor).
pub fn age_token(last: Timestamp, cx: &TextContext<'_>) -> String {
    todo!("M1: R§5.4")
}

/// The flyout card's age (F§4.2): R§5.4's boundaries in long form — `Just now` (0 min),
/// `1 min ago` … `59 min ago`, `1 h ago` … `47 h ago`, `2 d ago` ….
pub fn age_long(last: Timestamp, cx: &TextContext<'_>) -> String {
    todo!("M1: F§4.2 age")
}
