//! Percent and numbers (R§5.6) and balance durations (R§5.5, F§4.5).

use super::locale::{LocaleFormat, PercentPattern};
use super::strings::{Msg, PluralMsg, Strings, count, text};
use crate::consts::{
    BAL_DURATION_DAYS_BELOW_DAYS, BAL_DURATION_HOURS_BELOW_DAYS, BAL_DURATION_WEEKS_BELOW_DAYS,
};
use crate::stats::BalanceStats;
use crate::util::round_half_up;

/// The narrow no-break space of spaced percent patterns (R§5.6).
const NNBSP: char = '\u{202F}';

/// A percent with the locale's pattern (R§5.6): en `42%`; sv-SE `42 %` with U+202F. Digits are
/// tabular in the fonts used, so no padding is added.
pub fn percent(pct: u8, locale: &LocaleFormat) -> String {
    match locale.percent {
        PercentPattern::NumberPercent => format!("{pct}%"),
        PercentPattern::NumberSpacePercent => format!("{pct}{NNBSP}%"),
        PercentPattern::PercentNumber => format!("%{pct}"),
        PercentPattern::PercentSpaceNumber => format!("%{NNBSP}{pct}"),
    }
}

/// A share shown as a whole percent, half up (`e = 0.5533` → `55` in `55% of time gone`;
/// `p = 0.621` → `62` in `on course for 62% at reset`, clamped to 99 there, R§8.2).
pub fn share_pct(share: f64) -> u8 {
    if share.is_nan() {
        return 0;
    }
    // Clamped to the u8 range first, so the cast is exact.
    round_half_up(share * 100.0 + 1e-9).clamp(0.0, 255.0) as u8
}

/// An amount as a whole number with the locale's digit grouping: `1,240`; sv-SE `1 240` with
/// U+202F (R§5.6). Credits are whole; fractions are rounded half up.
pub fn amount(value: f64, locale: &LocaleFormat) -> String {
    let rounded = if value.is_finite() {
        round_half_up(value)
    } else {
        0.0
    };
    // `as` saturates; balances never come near the i64 range.
    let whole = rounded as i64;
    let digits = whole.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * 3 + 1);
    if whole < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(&locale.group_separator);
        }
        out.push(ch);
    }
    out
}

/// A balance duration in one unit, floored: hours below 2 days (≥ 1), days below 14, weeks
/// below 63 days; `None` from 63 days (R§5.5).
enum BalanceSpan {
    Hours(i64),
    Days(i64),
    Weeks(i64),
    MoreThanEightWeeks,
}

fn balance_span(days_left: f64) -> BalanceSpan {
    // `as` truncates toward zero and saturates; `days_left` is never negative.
    if days_left < BAL_DURATION_HOURS_BELOW_DAYS {
        BalanceSpan::Hours(((days_left * 24.0).floor() as i64).max(1))
    } else if days_left < BAL_DURATION_DAYS_BELOW_DAYS {
        BalanceSpan::Days(days_left.floor() as i64)
    } else if days_left < BAL_DURATION_WEEKS_BELOW_DAYS {
        BalanceSpan::Weeks((days_left / 7.0).floor() as i64)
    } else {
        BalanceSpan::MoreThanEightWeeks
    }
}

/// The readout's balance duration after `≈` (R§5.5): `days_left < 2` → `Nh` (floor hours, ≥ 1);
/// `< 14` → `Nd` (floor); `< 63` → `Nw` (floor weeks); `≥ 63` → `9w+`. `None` (shown as `–`) when
/// there was no burn in 7 days or less than 48 h of history.
pub fn balance_duration(stats: &BalanceStats, strings: &dyn Strings) -> Option<String> {
    let n = |msg, n: i64| text(strings, msg, &[("n", &n.to_string())]);
    Some(match balance_span(stats.days_left?) {
        BalanceSpan::Hours(h) => n(Msg::BalanceHours, h),
        BalanceSpan::Days(d) => n(Msg::BalanceDays, d),
        BalanceSpan::Weeks(w) => n(Msg::BalanceWeeks, w),
        BalanceSpan::MoreThanEightWeeks => strings.template(Msg::BalanceMax).to_owned(),
    })
}

/// The flyout's balance forecast (F§4.5): `≈ 9 days left`, `≈ 20 hours left`, `≈ 6 weeks left`,
/// `More than 8 weeks left`, `No use in 7 days`, `Not enough history yet` (< 48 h); thresholds
/// and flooring as R§5.5. EXHAUSTED is handled by the caller (`Empty`).
pub fn balance_forecast_long(stats: &BalanceStats, strings: &dyn Strings) -> String {
    match stats.days_left.map(balance_span) {
        Some(BalanceSpan::MoreThanEightWeeks) => strings.template(Msg::FlyoutBalanceMax).to_owned(),
        Some(_) => match balance_duration_words(stats, strings) {
            Some(words) => text(strings, Msg::FlyoutBalanceLeft, &[("duration", &words)]),
            None => strings.template(Msg::FlyoutBalanceMax).to_owned(),
        },
        None if !stats.valid => strings.template(Msg::FlyoutBalanceNoHistory).to_owned(),
        None => strings.template(Msg::FlyoutBalanceNoUse).to_owned(),
    }
}

/// The hover card's balance duration words (`9 days`, `20 hours`, `6 weeks`) for
/// `lasts ≈ {duration}` (R§8.2). From 63 days the weeks keep counting (`10 weeks`): the card has
/// room for the number the readout abbreviates as `9w+`.
pub fn balance_duration_words(stats: &BalanceStats, strings: &dyn Strings) -> Option<String> {
    let days_left = stats.days_left?;
    Some(match balance_span(days_left) {
        BalanceSpan::Hours(h) => count(strings, PluralMsg::UnitHour, h),
        BalanceSpan::Days(d) => count(strings, PluralMsg::UnitDay, d),
        BalanceSpan::Weeks(w) => count(strings, PluralMsg::UnitWeek, w),
        BalanceSpan::MoreThanEightWeeks => count(
            strings,
            PluralMsg::UnitWeek,
            (days_left / 7.0).floor() as i64,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::English;
    use jiff::tz::TimeZone;

    fn stats(days_left: Option<f64>, valid: bool) -> BalanceStats {
        BalanceStats {
            reference: 100.0,
            used_share: 0.5,
            burn_per_day: if days_left.is_some() { 1.0 } else { 0.0 },
            valid,
            days_left,
            t_out: None,
            exhausted: false,
        }
    }

    #[test]
    fn percent_patterns() {
        let gb = LocaleFormat::en_gb(TimeZone::UTC);
        let sv = LocaleFormat::sv_se(TimeZone::UTC);
        assert_eq!(percent(42, &gb), "42%");
        assert_eq!(percent(42, &sv), "42\u{202F}%");
        let mut other = gb.clone();
        other.percent = PercentPattern::PercentNumber;
        assert_eq!(percent(7, &other), "%7");
        assert_eq!(share_pct(0.5533), 55);
        assert_eq!(share_pct(0.621), 62);
        assert_eq!(share_pct(-1.0), 0);
        assert_eq!(share_pct(9.0), 255);
    }

    #[test]
    fn amounts_are_grouped() {
        let gb = LocaleFormat::en_gb(TimeZone::UTC);
        let sv = LocaleFormat::sv_se(TimeZone::UTC);
        assert_eq!(amount(1240.0, &gb), "1,240");
        assert_eq!(amount(1240.0, &sv), "1\u{202F}240");
        assert_eq!(amount(999.5, &gb), "1,000");
        assert_eq!(amount(1_234_567.0, &gb), "1,234,567");
        assert_eq!(amount(-1500.0, &gb), "-1,500");
        assert_eq!(amount(0.0, &gb), "0");
        assert_eq!(amount(f64::NAN, &gb), "0");
    }

    #[test]
    fn balance_durations_by_size() {
        let d = |x| balance_duration(&stats(Some(x), true), &English);
        assert_eq!(d(0.01).as_deref(), Some("1h"));
        assert_eq!(d(1.99).as_deref(), Some("47h"));
        assert_eq!(d(2.0).as_deref(), Some("2d"));
        assert_eq!(d(13.9).as_deref(), Some("13d"));
        assert_eq!(d(14.0).as_deref(), Some("2w"));
        assert_eq!(d(62.9).as_deref(), Some("8w"));
        assert_eq!(d(63.0).as_deref(), Some("9w+"));
        assert_eq!(balance_duration(&stats(None, true), &English), None);

        let long = |s: BalanceStats| balance_forecast_long(&s, &English);
        assert_eq!(long(stats(Some(9.12), true)), "≈ 9 days left");
        assert_eq!(long(stats(Some(0.85), true)), "≈ 20 hours left");
        assert_eq!(long(stats(Some(45.0), true)), "≈ 6 weeks left");
        assert_eq!(long(stats(Some(70.0), true)), "More than 8 weeks left");
        assert_eq!(long(stats(None, true)), "No use in 7 days");
        assert_eq!(long(stats(None, false)), "Not enough history yet");

        let words = |x| balance_duration_words(&stats(Some(x), true), &English);
        assert_eq!(words(1.0 / 24.0).as_deref(), Some("1 hour"));
        assert_eq!(words(9.12).as_deref(), Some("9 days"));
        assert_eq!(words(7.0).as_deref(), Some("7 days"));
        assert_eq!(words(70.0).as_deref(), Some("10 weeks"));
    }
}
