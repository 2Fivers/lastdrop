//! Instants and durations (R§5.0), time points (R§5.2), countdowns (R§5.3), ages (R§5.4) and the
//! card/flyout time rule (R§8.2, F§4.2, F§4.4).
//!
//! One rule for every surface (R§5.0): `now` is shown as `floor_min(now)` (as the taskbar clock
//! shows it), every other instant as `round_min(t)` (half up: a reset at `14:59:59.94` shows
//! `15:00`), and every duration shown is `round_min(t) − floor_min(now)`, so the clock, the shown
//! time and the shown duration always add up.

use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};

use super::TextContext;
use super::locale::{DayMonthOrder, LocaleFormat};
use super::strings::{Msg, Plural, Strings, text};
use crate::consts::{COUNTDOWN_HOURS_BELOW, COUNTDOWN_MAX_MIN, TIME_OF_DAY_MAX};
use crate::stats::runway_minutes;

const NANOS_PER_MIN: i128 = 60_000_000_000;
const MINS_PER_HOUR: i64 = 60;
const MINS_PER_DAY: i64 = 24 * 60;
const MINS_PER_WEEK: i64 = 7 * MINS_PER_DAY;

/// A timestamp from nanoseconds, or `fallback` when out of range.
fn from_nanos(ns: i128, fallback: Timestamp) -> Timestamp {
    Timestamp::from_nanosecond(ns).unwrap_or(fallback)
}

/// `floor_min(t)`: `t` truncated to the minute (R§5.0).
pub fn floor_min(t: Timestamp) -> Timestamp {
    let ns = t.as_nanosecond();
    from_nanos(ns.div_euclid(NANOS_PER_MIN) * NANOS_PER_MIN, t)
}

/// `round_min(t)`: `t` rounded to the nearest minute, half up (R§5.0).
pub fn round_min(t: Timestamp) -> Timestamp {
    let ns = t.as_nanosecond();
    from_nanos(
        (ns + NANOS_PER_MIN / 2).div_euclid(NANOS_PER_MIN) * NANOS_PER_MIN,
        floor_min(t),
    )
}

/// A duration as shown: `round_min(t) − floor_min(now)` (R§5.0). Negative when `t` is past.
pub fn shown_duration(now: Timestamp, t: Timestamp) -> SignedDuration {
    round_min(t).duration_since(floor_min(now))
}

/// An age as shown: `floor_min(now) − round_min(last_success)` (R§5.0).
pub fn shown_age(now: Timestamp, last: Timestamp) -> SignedDuration {
    floor_min(now).duration_since(round_min(last))
}

/// The local calendar date of `t` in the locale's time zone.
fn local_date(t: Timestamp, locale: &LocaleFormat) -> Date {
    t.to_zoned(locale.tz.clone()).date()
}

/// Whole calendar days from the local date of `from` to the local date of `to`.
fn days_between(from: Timestamp, to: Timestamp, locale: &LocaleFormat) -> i64 {
    local_date(from, locale)
        .duration_until(local_date(to, locale))
        .as_hours()
        .div_euclid(24)
}

/// The locale's abbreviated weekday of `t` (`Mon`, sv-SE `mån`).
fn weekday_abbrev(t: Timestamp, locale: &LocaleFormat) -> &str {
    let day = local_date(t, locale).weekday().to_monday_zero_offset();
    &locale.weekdays_abbrev[usize::try_from(day).unwrap_or(0) % 7]
}

/// The locale's full weekday of `t` (`Monday`).
fn weekday_full(t: Timestamp, locale: &LocaleFormat) -> &str {
    let day = local_date(t, locale).weekday().to_monday_zero_offset();
    &locale.weekdays_full[usize::try_from(day).unwrap_or(0) % 7]
}

/// Day and abbreviated month in the locale's order (`1 Nov`, `Nov 1`).
fn day_month(t: Timestamp, locale: &LocaleFormat) -> String {
    let date = local_date(t, locale);
    let month = usize::try_from(date.month() - 1).unwrap_or(0) % 12;
    let month = &locale.months_abbrev[month];
    match locale.day_month {
        DayMonthOrder::DayMonth => format!("{} {month}", date.day()),
        DayMonthOrder::MonthDay => format!("{month} {}", date.day()),
    }
}

/// `{n}` as a template argument.
fn num(msg: Msg, n: i64, strings: &dyn Strings) -> String {
    text(strings, msg, &[("n", &n.to_string())])
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
    let delta = shown_duration(cx.now, t);
    match cx.time_style {
        super::TimeStyle::Relative => {
            let mins = delta.as_mins();
            if mins < MINS_PER_HOUR {
                num(Msg::TokenRelMin, mins.max(1), cx.strings)
            } else if mins < MINS_PER_DAY {
                num(Msg::TokenRelHours, mins / MINS_PER_HOUR, cx.strings)
            } else {
                num(Msg::TokenRelDays, mins / MINS_PER_DAY, cx.strings)
            }
        }
        super::TimeStyle::Clock => {
            if delta < TIME_OF_DAY_MAX {
                return clock_time(t, cx.locale, false);
            }
            let shown = round_min(t);
            let days = days_between(floor_min(cx.now), shown, cx.locale);
            if days <= 0 {
                cx.strings.template(Msg::Today).to_owned()
            } else if days <= 6 {
                weekday_abbrev(shown, cx.locale).to_owned()
            } else {
                num(Msg::TokenDays, days, cx.strings)
            }
        }
    }
}

/// The locale's short time of `t` (after `round_min`): `14:56`; with `ampm`, the 12-hour
/// designator where the locale has one (`2:56 PM`), else without (`2:56`) (R§5.2).
pub fn clock_time(t: Timestamp, locale: &LocaleFormat, ampm: bool) -> String {
    let local = round_min(t).to_zoned(locale.tz.clone());
    let (hour, minute) = (local.hour(), local.minute());
    let clock = &locale.clock;
    let shown_hour = if clock.hour24 {
        hour
    } else {
        match hour % 12 {
            0 => 12,
            h => h,
        }
    };
    let time = if clock.leading_zero {
        format!("{shown_hour:02}{}{minute:02}", clock.separator)
    } else {
        format!("{shown_hour}{}{minute:02}", clock.separator)
    };
    let designator = if hour < 12 { &clock.am } else { &clock.pm };
    if !ampm || clock.hour24 || designator.is_empty() {
        time
    } else if clock.designator_first {
        format!("{designator} {time}")
    } else {
        format!("{time} {designator}")
    }
}

/// Times in the hover card and the flyout (R§8.2, one rule with `round_min`):
/// Δ < 12 h → the full short time with AM/PM (`2:56 PM`); same local date → `Today 18:00`;
/// date ≤ today + 6 → weekday + time (`Mon 09:00`); later → weekday + date, no time
/// (`Sun 1 Nov`).
///
/// Past instants (`Last reading …`, `showing …`, `Used up since …`) follow the mirrored rule, so
/// a reading from two days ago reads `Thu 12:42`, never a bare `12:42`.
pub fn card_time(t: Timestamp, cx: &TextContext<'_>) -> String {
    if shown_duration(cx.now, t).abs() < TIME_OF_DAY_MAX {
        return clock_time(t, cx.locale, true);
    }
    let shown = round_min(t);
    let days = days_between(floor_min(cx.now), shown, cx.locale).abs();
    if days == 0 {
        format!(
            "{} {}",
            cx.strings.template(Msg::Today),
            clock_time(t, cx.locale, true)
        )
    } else if days <= 6 {
        format!(
            "{} {}",
            weekday_abbrev(shown, cx.locale),
            clock_time(t, cx.locale, true)
        )
    } else {
        card_date(t, cx)
    }
}

/// Weekday + date without a time (`Mon 19 Oct`, `Mon Oct 19`): the balance "until" line
/// (R§8.2, F§4.5).
pub fn card_date(t: Timestamp, cx: &TextContext<'_>) -> String {
    let shown = round_min(t);
    format!(
        "{} {}",
        weekday_abbrev(shown, cx.locale),
        day_month(shown, cx.locale)
    )
}

/// A time point in a UI Automation sentence: `at 2:56 PM` within 12 h, else full weekday + time
/// (`Monday 9:00 AM`) (R§11 examples).
pub fn spoken_time(t: Timestamp, cx: &TextContext<'_>) -> String {
    if shown_duration(cx.now, t).abs() < TIME_OF_DAY_MAX {
        text(
            cx.strings,
            Msg::A11yAt,
            &[("time", &clock_time(t, cx.locale, true))],
        )
    } else {
        spoken_point(t, cx)
    }
}

/// A time point in a UI Automation sentence without the preposition, for templates that bring
/// their own (`runs out at about {time}`): `2:56 PM` within 12 h, `Monday 9:00 AM` within a week,
/// later `Sunday 1 Nov`.
pub(crate) fn spoken_point(t: Timestamp, cx: &TextContext<'_>) -> String {
    if shown_duration(cx.now, t).abs() < TIME_OF_DAY_MAX {
        return clock_time(t, cx.locale, true);
    }
    let shown = round_min(t);
    let days = days_between(floor_min(cx.now), shown, cx.locale).abs();
    if days <= 6 {
        format!(
            "{} {}",
            weekday_full(shown, cx.locale),
            clock_time(t, cx.locale, true)
        )
    } else {
        format!(
            "{} {}",
            weekday_full(shown, cx.locale),
            day_month(shown, cx.locale)
        )
    }
}

/// Long durations, hover card and flyout (R§5.0): `< 60 min` → `N min`; `< 24 h` → `H h M min`
/// (`2 h` when M = 0); `< 7 d` → `D d H h` (`3 d` when H = 0); else `D d`. `d` is a shown
/// duration ([`shown_duration`]), whole minutes.
pub fn long_duration(d: SignedDuration, strings: &dyn Strings) -> String {
    let mins = d.as_mins().max(0);
    if mins < MINS_PER_HOUR {
        return num(Msg::DurationMin, mins, strings);
    }
    if mins < MINS_PER_DAY {
        let (h, m) = (mins / MINS_PER_HOUR, mins % MINS_PER_HOUR);
        let h = h.to_string();
        return if m == 0 {
            text(strings, Msg::DurationHours, &[("h", &h)])
        } else {
            text(
                strings,
                Msg::DurationHoursMin,
                &[("h", &h), ("m", &m.to_string())],
            )
        };
    }
    let days = (mins / MINS_PER_DAY).to_string();
    if mins < MINS_PER_WEEK {
        let h = (mins % MINS_PER_DAY) / MINS_PER_HOUR;
        if h > 0 {
            return text(
                strings,
                Msg::DurationDaysHours,
                &[("d", &days), ("h", &h.to_string())],
            );
        }
    }
    text(strings, Msg::DurationDays, &[("d", &days)])
}

/// `n` units in words with the language's plural form.
fn plural(n: i64, one: Msg, other: Msg, strings: &dyn Strings) -> String {
    let msg = match strings.plural(n) {
        Plural::One => one,
        Plural::Other => other,
    };
    num(msg, n, strings)
}

/// A duration in words for UI Automation (`12 minutes`, `6 hours`, `9 days`): the largest
/// whole unit, with the language's plural forms (R§11).
///
/// The units switch where the countdown's do (R§5.3): minutes below an hour, hours below
/// 48 hours, then days, so a sentence says what the readout shows (`6 h left` → `6 hours`).
pub fn spoken_duration(d: SignedDuration, strings: &dyn Strings) -> String {
    let mins = d.as_mins().max(1);
    if mins < MINS_PER_HOUR {
        plural(mins, Msg::UnitMinuteOne, Msg::UnitMinuteOther, strings)
    } else if mins < COUNTDOWN_HOURS_BELOW.as_mins() {
        plural(
            mins / MINS_PER_HOUR,
            Msg::UnitHourOne,
            Msg::UnitHourOther,
            strings,
        )
    } else {
        plural(
            mins / MINS_PER_DAY,
            Msg::UnitDayOne,
            Msg::UnitDayOther,
            strings,
        )
    }
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
    let n = n_min.max(1);
    let (min, hours, days) = match form {
        CountdownForm::Left => (Msg::CountdownMin, Msg::CountdownHours, Msg::CountdownDays),
        CountdownForm::Compact => (
            Msg::CountdownCompactMin,
            Msg::CountdownCompactHours,
            Msg::CountdownCompactDays,
        ),
    };
    if n <= COUNTDOWN_MAX_MIN {
        num(min, n, strings)
    } else if n < COUNTDOWN_HOURS_BELOW.as_mins() {
        num(hours, n / MINS_PER_HOUR, strings)
    } else {
        num(days, n / MINS_PER_DAY, strings)
    }
}

/// Whether a run-out shows as a countdown (R§5.3): always when CRITICAL, else when `N ≤ 59`.
pub(crate) fn shows_countdown(n_min: i64, critical: bool) -> bool {
    critical || n_min <= COUNTDOWN_MAX_MIN
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
    let n = runway_minutes(cx.now, t_out);
    if shows_countdown(n, critical) {
        countdown(n, CountdownForm::Left, cx.strings)
    } else {
        text(cx.strings, Msg::Out, &[("time", &time_token(t_out, cx))])
    }
}

/// The readout's `AGE` (R§5.4): `floor_min(now) − round_min(last)`; < 60 min → `Nm ago`
/// (N ≥ 1); < 48 h → `Nh ago` (floor); else `Nd ago` (floor).
pub fn age_token(last: Timestamp, cx: &TextContext<'_>) -> String {
    let mins = shown_age(cx.now, last).as_mins();
    if mins < MINS_PER_HOUR {
        num(Msg::AgeMin, mins.max(1), cx.strings)
    } else if mins < COUNTDOWN_HOURS_BELOW.as_mins() {
        num(Msg::AgeHours, mins / MINS_PER_HOUR, cx.strings)
    } else {
        num(Msg::AgeDays, mins / MINS_PER_DAY, cx.strings)
    }
}

/// The flyout card's age (F§4.2): R§5.4's boundaries in long form — `Just now` (0 min),
/// `1 min ago` … `59 min ago`, `1 h ago` … `47 h ago`, `2 d ago` ….
pub fn age_long(last: Timestamp, cx: &TextContext<'_>) -> String {
    let mins = shown_age(cx.now, last).as_mins();
    if mins <= 0 {
        cx.strings.template(Msg::FlyoutAgeNow).to_owned()
    } else {
        age_long_min1(mins, cx.strings)
    }
}

/// [`age_long`] for an age of at least one minute (`Updated 1 min ago` in the hover card never
/// says "just now").
pub(crate) fn age_long_min1(mins: i64, strings: &dyn Strings) -> String {
    let mins = mins.max(1);
    if mins < MINS_PER_HOUR {
        num(Msg::FlyoutAgeMin, mins, strings)
    } else if mins < COUNTDOWN_HOURS_BELOW.as_mins() {
        num(Msg::FlyoutAgeHours, mins / MINS_PER_HOUR, strings)
    } else {
        num(Msg::FlyoutAgeDays, mins / MINS_PER_DAY, strings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::{English, TimeStyle};
    use jiff::tz::{self, TimeZone};

    fn tz() -> TimeZone {
        TimeZone::fixed(tz::offset(2))
    }

    fn at(day: i8, hour: i8, minute: i8, second: i8) -> Timestamp {
        jiff::civil::date(2026, 10, day)
            .at(hour, minute, second, 0)
            .to_zoned(tz())
            .expect("valid")
            .timestamp()
    }

    fn cx(now: Timestamp, locale: &LocaleFormat) -> TextContext<'_> {
        TextContext {
            now,
            time_style: TimeStyle::Clock,
            locale,
            strings: &English,
        }
    }

    #[test]
    fn minute_rounding() {
        let t = at(10, 12, 42, 29);
        assert_eq!(floor_min(t), at(10, 12, 42, 0));
        assert_eq!(round_min(t), at(10, 12, 42, 0));
        assert_eq!(round_min(at(10, 12, 42, 30)), at(10, 12, 43, 0));
        let before_epoch = Timestamp::from_second(-90).expect("valid");
        assert_eq!(floor_min(before_epoch).as_second(), -120);
        assert_eq!(round_min(before_epoch).as_second(), -60);
    }

    #[test]
    fn clock_formats_for_12_and_24_hour_locales() {
        let gb = LocaleFormat::en_gb(tz());
        let us = LocaleFormat::en_us(tz());
        let sv = LocaleFormat::sv_se(tz());
        let midnight = at(11, 0, 5, 0);
        let noon = at(11, 12, 0, 0);
        assert_eq!(clock_time(midnight, &gb, true), "00:05");
        assert_eq!(clock_time(midnight, &us, false), "12:05");
        assert_eq!(clock_time(midnight, &us, true), "12:05 AM");
        assert_eq!(clock_time(noon, &us, true), "12:00 PM");
        assert_eq!(clock_time(at(11, 9, 0, 0), &us, true), "9:00 AM");
        assert_eq!(clock_time(at(11, 9, 0, 0), &sv, true), "09:00");
        let mut first = us.clone();
        first.clock.designator_first = true;
        assert_eq!(clock_time(noon, &first, true), "PM 12:00");
    }

    #[test]
    fn card_times() {
        let (gb, us) = (LocaleFormat::en_gb(tz()), LocaleFormat::en_us(tz()));
        let now = at(10, 12, 42, 0);
        assert_eq!(card_time(at(10, 14, 56, 0), &cx(now, &us)), "2:56 PM");
        let early = at(10, 0, 30, 0);
        assert_eq!(card_time(at(10, 18, 0, 0), &cx(early, &gb)), "Today 18:00");
        assert_eq!(card_time(at(12, 9, 0, 0), &cx(now, &gb)), "Mon 09:00");
        let nov1 = jiff::civil::date(2026, 11, 1)
            .at(0, 0, 0, 0)
            .to_zoned(tz())
            .expect("valid")
            .timestamp();
        assert_eq!(card_time(nov1, &cx(now, &gb)), "Sun 1 Nov");
        assert_eq!(card_date(nov1, &cx(now, &us)), "Sun Nov 1");
        let sv = LocaleFormat::sv_se(tz());
        assert_eq!(card_date(at(19, 9, 0, 0), &cx(now, &sv)), "mån 19 okt");
        // Past instants mirror the rule: 47 min ago is a time, two days ago a weekday + time.
        assert_eq!(card_time(at(10, 11, 55, 0), &cx(now, &gb)), "11:55");
        assert_eq!(card_time(at(10, 0, 5, 0), &cx(now, &gb)), "Today 00:05");
        assert_eq!(card_time(at(8, 12, 42, 0), &cx(now, &gb)), "Thu 12:42");
        assert_eq!(card_time(at(1, 12, 42, 0), &cx(now, &gb)), "Thu 1 Oct");
    }

    #[test]
    fn spoken_times_and_durations() {
        let us = LocaleFormat::en_us(tz());
        let now = at(10, 12, 42, 0);
        let c = cx(now, &us);
        assert_eq!(spoken_time(at(10, 14, 56, 0), &c), "at 2:56 PM");
        assert_eq!(spoken_time(at(12, 9, 0, 0), &c), "Monday 9:00 AM");
        assert_eq!(spoken_point(at(10, 15, 20, 0), &c), "3:20 PM");
        assert_eq!(spoken_point(at(25, 9, 0, 0), &c), "Sunday Oct 25");
        let m = SignedDuration::from_mins;
        assert_eq!(spoken_duration(m(1), &English), "1 minute");
        assert_eq!(spoken_duration(m(12), &English), "12 minutes");
        assert_eq!(spoken_duration(m(60), &English), "1 hour");
        assert_eq!(spoken_duration(m(370), &English), "6 hours");
        assert_eq!(spoken_duration(m(13_130), &English), "9 days");
        assert_eq!(spoken_duration(m(0), &English), "1 minute");
    }

    #[test]
    fn long_durations_drop_zero_parts() {
        let m = SignedDuration::from_mins;
        assert_eq!(long_duration(m(0), &English), "0 min");
        assert_eq!(long_duration(m(-5), &English), "0 min");
        assert_eq!(long_duration(m(120), &English), "2 h");
        assert_eq!(long_duration(m(3 * 1440), &English), "3 d");
        assert_eq!(long_duration(m(3 * 1440 + 61), &English), "3 d 1 h");
        assert_eq!(long_duration(m(7 * 1440 + 61), &English), "7 d");
    }

    #[test]
    fn relative_tokens() {
        let gb = LocaleFormat::en_gb(tz());
        let now = at(10, 12, 42, 0);
        let c = TextContext {
            time_style: TimeStyle::Relative,
            ..cx(now, &gb)
        };
        assert_eq!(time_token(at(10, 13, 27, 0), &c), "45m");
        assert_eq!(time_token(at(10, 12, 42, 10), &c), "1m");
        assert_eq!(time_token(at(10, 12, 30, 0), &c), "1m");
    }

    #[test]
    fn ages_in_long_form() {
        let gb = LocaleFormat::en_gb(tz());
        let now = at(10, 14, 50, 0);
        let c = cx(now, &gb);
        assert_eq!(age_long(at(10, 14, 49, 50), &c), "Just now");
        assert_eq!(age_long(at(10, 14, 49, 0), &c), "1 min ago");
        assert_eq!(age_long(at(10, 12, 50, 0), &c), "2 h ago");
        assert_eq!(age_long(at(8, 14, 50, 0), &c), "2 d ago");
    }
}
