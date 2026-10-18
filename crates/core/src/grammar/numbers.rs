//! Percent and numbers (R§5.6), a balance's unit after its amount, and balance durations
//! (R§5.5, F§4.5).

use jiff::{SignedDuration, Timestamp};

use super::locale::{LocaleFormat, PercentPattern};
use super::strings::{Msg, PluralMsg, Strings, count, plural_form, text};
use crate::consts::{
    BAL_DURATION_DAYS_BELOW_DAYS, BAL_DURATION_HOURS_BELOW_DAYS, BAL_DURATION_WEEKS_BELOW_DAYS,
    COUNTDOWN_MAX_MIN,
};
use crate::model::Unit;
use crate::stats::{BalanceStats, runway_minutes};
use crate::util::{duration_from_hours, round_half_up};

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

/// The whole number [`amount`] shows for `value`: rounded half up, 0 when it is not finite.
fn shown_whole(value: f64) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    // `as` saturates; balances never come near the i64 range.
    round_half_up(value) as i64
}

/// An amount as a whole number with the locale's digit grouping: `1,240`; sv-SE `1 240` with
/// U+202F (R§5.6). Credits are whole; fractions are rounded half up.
pub fn amount(value: f64, locale: &LocaleFormat) -> String {
    let whole = shown_whole(value);
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

/// The word of a balance's unit after an amount of `value`, in the plural form of the number
/// [`amount`] shows for it, so the two always agree: 1 and 1.4 show `1 credit`, 1.5 `2 credits`,
/// 0 `0 credits` (French `0 crédit`: CLDR counts 0 with the singular). A currency code has one
/// form (`1 USD`).
pub fn unit_word(unit: &Unit, value: f64, strings: &dyn Strings) -> String {
    match unit {
        Unit::Credits => {
            plural_form(strings, PluralMsg::UnitCredits, shown_whole(value)).to_owned()
        }
        Unit::Currency { code } => code.clone(),
    }
}

/// An amount followed by its unit (`1,240 credits`, `1 credit`, `12 USD`), for the texts that
/// show the two as one value.
pub fn amount_with_unit(
    value: f64,
    unit: &Unit,
    locale: &LocaleFormat,
    strings: &dyn Strings,
) -> String {
    format!(
        "{} {}",
        amount(value, locale),
        unit_word(unit, value, strings)
    )
}

/// `value` with one decimal in the locale's decimal separator (`4.2`, sv-SE `4,2`), rounded half
/// up; a whole number without it (`5`).
fn one_decimal(value: f64, locale: &LocaleFormat) -> String {
    let tenths = round_half_up(value * 10.0);
    // `as` saturates; counts and prices never come near the i64 range.
    let tenths = tenths as i64;
    if tenths % 10 == 0 {
        return (tenths / 10).to_string();
    }
    format!(
        "{}{}{}",
        tenths / 10,
        locale.decimal_separator,
        (tenths % 10).unsigned_abs()
    )
}

/// A token count, compact (tokens-spec §3): `950`, `12K`, `4.2M`, `5M`, `1.3B`; one decimal below
/// 10 unless it is 0, none from 10, the suffixes from the strings (Swedish `12 tn`).
pub fn token_count(n: u64, locale: &LocaleFormat, strings: &dyn Strings) -> String {
    const UNITS: [(f64, Msg); 3] = [
        (1e3, Msg::HistoryTokensCountK),
        (1e6, Msg::HistoryTokensCountM),
        (1e9, Msg::HistoryTokensCountB),
    ];
    // Precision is irrelevant at these magnitudes: the result is rounded to two digits.
    let value = n as f64;
    if value < 1e3 {
        return amount(value, locale);
    }
    let mut unit = 0;
    while unit + 1 < UNITS.len() && round_half_up(value / UNITS[unit].0) >= 1e3 {
        unit += 1;
    }
    let (size, msg) = UNITS[unit];
    let scaled = value / size;
    let shown = if round_half_up(scaled * 10.0) < 100.0 {
        one_decimal(scaled, locale)
    } else {
        amount(scaled, locale)
    };
    text(strings, msg, &[("n", &shown)])
}

/// An amount of money (tokens-spec §3): two decimals below 100, whole and grouped from 100, the
/// ISO 4217 code after it: `12.40 USD`, `1,240 USD`; sv-SE `12,40 USD`.
pub fn money(value: f64, code: &str, locale: &LocaleFormat) -> String {
    let value = if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    };
    let cents = round_half_up(value * 100.0);
    let number = if cents < 10_000.0 {
        // `as` is exact: below 10,000.
        let cents = cents as i64;
        format!(
            "{}{}{:02}",
            cents / 100,
            locale.decimal_separator,
            cents % 100
        )
    } else {
        amount(value, locale)
    };
    format!("{number} {code}")
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

/// The countdown's minutes when a balance runs out within them: `N` of R§3.5 while `N ≤ 59`, the
/// minutes the readout's `11 min left` and the verdict's `runs out in 11 min` show. Below an
/// hour the words count these instead of R§5.5's floor of 1 hour, which only the readout's
/// `≈1h` keeps (a balance that close is CRITICAL, and the readout counts it down instead).
fn countdown_minutes(stats: &BalanceStats, now: Timestamp) -> Option<i64> {
    stats
        .t_out
        .map(|t| runway_minutes(now, t))
        .filter(|n| *n <= COUNTDOWN_MAX_MIN)
}

/// How long a balance lasts, for sentences (R§11): the countdown's minutes below an hour
/// ([`countdown_minutes`]), else `days_left`. `None` without a forecast, and for an empty
/// balance, which has no time left to speak of.
pub fn balance_left(stats: &BalanceStats, now: Timestamp) -> Option<SignedDuration> {
    let days_left = stats.days_left.filter(|_| !stats.exhausted)?;
    match countdown_minutes(stats, now) {
        Some(n) => Some(SignedDuration::from_mins(n)),
        None => duration_from_hours(days_left * 24.0),
    }
}

/// The flyout's balance forecast (F§4.5): `≈ 9 days left`, `≈ 20 hours left`, `≈ 6 weeks left`,
/// `More than 8 weeks left`, `No use in 7 days`, `Not enough history yet` (< 48 h); thresholds
/// and flooring as R§5.5, minutes below an hour (`≈ 11 minutes left`, [`countdown_minutes`]).
/// EXHAUSTED is handled by the caller (`Empty`).
pub fn balance_forecast_long(
    stats: &BalanceStats,
    now: Timestamp,
    strings: &dyn Strings,
) -> String {
    match stats.days_left.map(balance_span) {
        Some(BalanceSpan::MoreThanEightWeeks) => strings.template(Msg::FlyoutBalanceMax).to_owned(),
        Some(_) => match balance_duration_words(stats, now, strings) {
            Some(words) => text(strings, Msg::FlyoutBalanceLeft, &[("duration", &words)]),
            None => strings.template(Msg::FlyoutBalanceMax).to_owned(),
        },
        None if !stats.valid => strings.template(Msg::FlyoutBalanceNoHistory).to_owned(),
        None => strings.template(Msg::FlyoutBalanceNoUse).to_owned(),
    }
}

/// The hover card's balance duration words (`9 days`, `20 hours`, `6 weeks`) for
/// `lasts ≈ {duration}` (R§8.2), also in the flyout's forecast and in notices; `11 minutes`
/// below an hour ([`countdown_minutes`]). From 63 days the weeks keep counting (`10 weeks`): the
/// card has room for the number the readout abbreviates as `9w+`.
pub fn balance_duration_words(
    stats: &BalanceStats,
    now: Timestamp,
    strings: &dyn Strings,
) -> Option<String> {
    let days_left = stats.days_left?;
    if let Some(n) = countdown_minutes(stats, now) {
        return Some(count(strings, PluralMsg::UnitMinute, n));
    }
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
    use crate::util::add;
    use jiff::tz::TimeZone;

    /// On a minute, so a run-out `x` minutes away counts down `round(x)` minutes.
    fn now() -> Timestamp {
        Timestamp::from_second(1_800_000_000).expect("valid")
    }

    fn stats(days_left: Option<f64>, valid: bool) -> BalanceStats {
        BalanceStats {
            reference: 100.0,
            used_share: 0.5,
            burn_per_day: if days_left.is_some() { 1.0 } else { 0.0 },
            valid,
            days_left,
            t_out: days_left
                .and_then(|d| duration_from_hours(d * 24.0))
                .map(|d| add(now(), d)),
            exhausted: false,
        }
    }

    /// `x` minutes in days.
    fn mins(x: f64) -> f64 {
        x / 1440.0
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
    fn a_balance_unit_agrees_with_its_amount_in_every_language() {
        use crate::grammar::strings::Language;

        // The unit after 0, 1 and 2 credits.
        let units = [
            (Language::English, ["credits", "credit", "credits"]),
            (Language::Swedish, ["krediter", "kredit", "krediter"]),
            (Language::German, ["Credits", "Credit", "Credits"]),
            // CLDR counts 0 with the singular in French.
            (Language::French, ["crédit", "crédit", "crédits"]),
            (Language::Spanish, ["créditos", "crédito", "créditos"]),
        ];
        for (l, [zero, one, two]) in units {
            let (s, locale) = (l.strings(), LocaleFormat::of_language(l, TimeZone::UTC));
            let with_unit = |v: f64| amount_with_unit(v, &Unit::Credits, &locale, s);
            for (n, unit) in [(0.0, zero), (1.0, one), (2.0, two)] {
                assert_eq!(unit_word(&Unit::Credits, n, s), unit, "{}: {n}", l.tag());
                assert_eq!(with_unit(n), format!("{n} {unit}"), "{}", l.tag());
            }
            // The number as shown picks the form: 1.4 and 0.5 show 1, 1.5 shows 2, 0.4 shows 0.
            assert_eq!(with_unit(1.4), format!("1 {one}"), "{}", l.tag());
            assert_eq!(with_unit(0.5), format!("1 {one}"), "{}", l.tag());
            assert_eq!(with_unit(1.5), format!("2 {two}"), "{}", l.tag());
            assert_eq!(with_unit(0.4), format!("0 {zero}"), "{}", l.tag());
            assert_eq!(with_unit(f64::NAN), format!("0 {zero}"), "{}", l.tag());
            // A debt of one takes the singular too (CLDR's rules count the size).
            assert_eq!(with_unit(-1.0), format!("-1 {one}"), "{}", l.tag());
            // A currency code has one form.
            let usd = Unit::Currency {
                code: "USD".to_owned(),
            };
            assert_eq!(amount_with_unit(1.0, &usd, &locale, s), "1 USD");
            assert_eq!(amount_with_unit(2.0, &usd, &locale, s), "2 USD");
        }
        let gb = LocaleFormat::en_gb(TimeZone::UTC);
        assert_eq!(
            amount_with_unit(1240.0, &Unit::Credits, &gb, &English),
            "1,240 credits"
        );
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

        let long = |s: BalanceStats| balance_forecast_long(&s, now(), &English);
        assert_eq!(long(stats(Some(9.12), true)), "≈ 9 days left");
        assert_eq!(long(stats(Some(0.85), true)), "≈ 20 hours left");
        assert_eq!(long(stats(Some(45.0), true)), "≈ 6 weeks left");
        assert_eq!(long(stats(Some(70.0), true)), "More than 8 weeks left");
        assert_eq!(long(stats(None, true)), "No use in 7 days");
        assert_eq!(long(stats(None, false)), "Not enough history yet");

        let words = |x| balance_duration_words(&stats(Some(x), true), now(), &English);
        assert_eq!(words(1.0 / 24.0).as_deref(), Some("1 hour"));
        assert_eq!(words(9.12).as_deref(), Some("9 days"));
        assert_eq!(words(7.0).as_deref(), Some("7 days"));
        assert_eq!(words(70.0).as_deref(), Some("10 weeks"));
    }

    #[test]
    fn below_an_hour_the_words_count_the_countdowns_minutes() {
        // 1 credit at 136 a day: 10.6 minutes, which the readout and the verdict count down as
        // `11 min` (`N`, R§3.5). The readout's token keeps R§5.5's floor of 1 hour.
        let last = stats(Some(1.0 / 136.0), true);
        assert_eq!(balance_duration(&last, &English).as_deref(), Some("1h"));
        assert_eq!(
            balance_forecast_long(&last, now(), &English),
            "≈ 11 minutes left"
        );
        assert_eq!(
            balance_duration_words(&last, now(), &English).as_deref(),
            Some("11 minutes")
        );
        assert_eq!(
            balance_left(&last, now()),
            Some(SignedDuration::from_mins(11))
        );
        let words = |x| balance_duration_words(&stats(Some(x), true), now(), &English);
        assert_eq!(words(mins(0.2)).as_deref(), Some("1 minute"));
        assert_eq!(words(mins(59.4)).as_deref(), Some("59 minutes"));
        // 59.6 minutes count down as 60: `1 h left`, so `1 hour`.
        assert_eq!(words(mins(59.6)).as_deref(), Some("1 hour"));
        // The minutes follow the clock: a minute later the same reading has one fewer.
        let later = add(now(), SignedDuration::from_mins(1));
        assert_eq!(
            balance_duration_words(&last, later, &English).as_deref(),
            Some("10 minutes")
        );

        // Above an hour a sentence says `days_left` as it did; an empty balance says nothing.
        let days = stats(Some(9.12), true);
        assert_eq!(balance_left(&days, now()), duration_from_hours(9.12 * 24.0));
        let empty = BalanceStats {
            exhausted: true,
            ..stats(Some(0.0), true)
        };
        assert_eq!(balance_left(&empty, now()), None);
        assert_eq!(balance_left(&stats(None, true), now()), None);
    }
}
