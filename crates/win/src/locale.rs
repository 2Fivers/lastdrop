//! The user's regional format for the text grammar (R§5.2, R§5.6): `GetLocaleInfoEx(
//! LOCALE_NAME_USER_DEFAULT, …)` for `LOCALE_SSHORTTIME`, `LOCALE_S1159`/`LOCALE_S2359`,
//! `LOCALE_SABBREVDAYNAME1..7`, `LOCALE_SDAYNAME1..7`, `LOCALE_SABBREVMONTHNAME1..12`,
//! `LOCALE_IPOSITIVEPERCENT`, `LOCALE_STHOUSAND`, `LOCALE_SDECIMAL`, and the day/month order from
//! `LOCALE_SMONTHDAY` (falling back to `LOCALE_SLONGDATE`, then `LOCALE_SSHORTDATE`: sv-SE's short
//! date is ISO `yyyy-MM-dd`, month first, while Swedes write `1 nov`). Re-read on
//! `WM_SETTINGCHANGE` "intl". The user's own overrides in Settings › Region are honoured.
//!
//! Anything Windows does not answer keeps the en-GB value of [`LocaleFormat::en_gb`].
//!
//! Threading: main thread.

use lastdrop_core::grammar::LocaleFormat;
use lastdrop_core::grammar::locale::{ClockFormat, DayMonthOrder, PercentPattern};
use lastdrop_core::jiff::tz::TimeZone;
use windows::Win32::Globalization::{
    GetLocaleInfoEx, LOCALE_IPOSITIVEPERCENT, LOCALE_S1159, LOCALE_S2359, LOCALE_SABBREVDAYNAME1,
    LOCALE_SABBREVMONTHNAME1, LOCALE_SDAYNAME1, LOCALE_SDECIMAL, LOCALE_SLONGDATE,
    LOCALE_SMONTHDAY, LOCALE_SNAME, LOCALE_SSHORTDATE, LOCALE_SSHORTTIME, LOCALE_STHOUSAND,
};
use windows::core::PCWSTR;

/// U+202F NARROW NO-BREAK SPACE: the grouping separator R§5.6 draws for space-grouping locales.
const NNBSP: &str = "\u{202F}";

/// Reads the user's regional format; `tz` is the system time zone (the app reads it with
/// `TimeZone::system()`).
pub fn read(tz: TimeZone) -> LocaleFormat {
    let mut format = LocaleFormat::en_gb(tz);
    if let Some(name) = info(LOCALE_SNAME) {
        format.name = name;
    }
    if let Some(pattern) = info(LOCALE_SSHORTTIME) {
        let (am, pm) = (format.clock.am.clone(), format.clock.pm.clone());
        format.clock = parse_short_time(&pattern, am, pm);
    }
    if let Some(am) = info(LOCALE_S1159) {
        format.clock.am = am;
    }
    if let Some(pm) = info(LOCALE_S2359) {
        format.clock.pm = pm;
    }
    for (i, slot) in format.weekdays_abbrev.iter_mut().enumerate() {
        if let Some(name) = info(LOCALE_SABBREVDAYNAME1 + i as u32).filter(|s| !s.is_empty()) {
            *slot = name;
        }
    }
    for (i, slot) in format.weekdays_full.iter_mut().enumerate() {
        if let Some(name) = info(LOCALE_SDAYNAME1 + i as u32).filter(|s| !s.is_empty()) {
            *slot = name;
        }
    }
    for (i, slot) in format.months_abbrev.iter_mut().enumerate() {
        if let Some(name) = info(LOCALE_SABBREVMONTHNAME1 + i as u32).filter(|s| !s.is_empty()) {
            *slot = name;
        }
    }
    if let Some(order) = [LOCALE_SMONTHDAY, LOCALE_SLONGDATE, LOCALE_SSHORTDATE]
        .into_iter()
        .find_map(|lctype| info(lctype).and_then(|p| day_month_order(&p)))
    {
        format.day_month = order;
    }
    if let Some(pattern) = info(LOCALE_IPOSITIVEPERCENT).and_then(|s| percent_pattern(&s)) {
        format.percent = pattern;
    }
    if let Some(sep) = info(LOCALE_STHOUSAND) {
        format.group_separator = group_separator(&sep);
    }
    if let Some(sep) = info(LOCALE_SDECIMAL).filter(|s| !s.is_empty()) {
        format.decimal_separator = sep;
    }
    format
}

/// One `GetLocaleInfoEx(LOCALE_NAME_USER_DEFAULT, lctype)` string; `None` on failure.
fn info(lctype: u32) -> Option<String> {
    // SAFETY: a size query (no buffer) for the user default locale (null name).
    let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), lctype, None) };
    if len <= 0 {
        log::debug!("locale: GetLocaleInfoEx({lctype:#x}) size query failed");
        return None;
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` has exactly the length Windows asked for.
    let written = unsafe { GetLocaleInfoEx(PCWSTR::null(), lctype, Some(&mut buffer)) };
    if written <= 0 {
        log::debug!("locale: GetLocaleInfoEx({lctype:#x}) failed");
        return None;
    }
    buffer.truncate(written as usize);
    while buffer.last() == Some(&0) {
        buffer.pop();
    }
    String::from_utf16(&buffer).ok()
}

/// A format pattern split into runs: `Field(letter, count)` for unquoted pattern letters, and
/// `Literal(text)` for everything else (quotes removed, `''` is a literal quote).
#[derive(Debug, PartialEq)]
enum Token {
    Field(char, usize),
    Literal(String),
}

fn tokenize(pattern: &str, letters: &[char]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut literal = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                literal.push('\'');
                continue;
            }
            for q in chars.by_ref() {
                if q == '\'' {
                    break;
                }
                literal.push(q);
            }
        } else if letters.contains(&c) {
            if !literal.is_empty() {
                tokens.push(Token::Literal(std::mem::take(&mut literal)));
            }
            let mut count = 1;
            while chars.peek() == Some(&c) {
                chars.next();
                count += 1;
            }
            tokens.push(Token::Field(c, count));
        } else {
            literal.push(c);
        }
    }
    if !literal.is_empty() {
        tokens.push(Token::Literal(literal));
    }
    tokens
}

/// `LOCALE_SSHORTTIME` → clock format (R§5.2): `H` → 24 h; `HH`/`hh` → leading zero; the text
/// between the hour and the minutes is the separator; `t` before the hour puts the designator
/// first. A pattern without an hour keeps 24 h, `HH:mm`.
fn parse_short_time(pattern: &str, am: String, pm: String) -> ClockFormat {
    let tokens = tokenize(pattern, &['h', 'H', 'm', 's', 't']);
    let hour = tokens.iter().enumerate().find_map(|(i, t)| match *t {
        Token::Field(letter @ ('h' | 'H'), count) => Some((i, letter == 'H', count >= 2)),
        _ => None,
    });
    let Some((hour, hour24, leading_zero)) = hour else {
        return ClockFormat {
            hour24: true,
            leading_zero: true,
            separator: ":".to_owned(),
            am,
            pm,
            designator_first: false,
        };
    };
    let minute = tokens[hour..]
        .iter()
        .position(|t| matches!(t, Token::Field('m', _)))
        .map(|i| i + hour);
    let separator = minute
        .map(|m| {
            tokens[hour + 1..m]
                .iter()
                .filter_map(|t| match t {
                    Token::Literal(s) => Some(s.as_str()),
                    Token::Field(..) => None,
                })
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ":".to_owned());
    let designator_first = tokens[..hour]
        .iter()
        .any(|t| matches!(t, Token::Field('t', _)));
    ClockFormat {
        hour24,
        leading_zero,
        separator,
        am,
        pm,
        designator_first,
    }
}

/// Day/month order of a date pattern: the first day number (`d`/`dd`, not the `ddd`/`dddd`
/// weekday names) against the first month (`M…`). `None` when either is missing.
fn day_month_order(pattern: &str) -> Option<DayMonthOrder> {
    let tokens = tokenize(pattern, &['d', 'M', 'y', 'g']);
    let day = tokens
        .iter()
        .position(|t| matches!(t, Token::Field('d', 1..=2)))?;
    let month = tokens
        .iter()
        .position(|t| matches!(t, Token::Field('M', _)))?;
    Some(if day < month {
        DayMonthOrder::DayMonth
    } else {
        DayMonthOrder::MonthDay
    })
}

/// `LOCALE_IPOSITIVEPERCENT`: 0 `# %`, 1 `#%`, 2 `%#`, 3 `% #`.
fn percent_pattern(value: &str) -> Option<PercentPattern> {
    match value.trim() {
        "0" => Some(PercentPattern::NumberSpacePercent),
        "1" => Some(PercentPattern::NumberPercent),
        "2" => Some(PercentPattern::PercentNumber),
        "3" => Some(PercentPattern::PercentSpaceNumber),
        _ => None,
    }
}

/// The grouping separator as drawn: a space or no-break space becomes U+202F (R§5.6: sv-SE
/// `1 240` with a narrow no-break space, which also never wraps).
fn group_separator(value: &str) -> String {
    match value {
        " " | "\u{00A0}" => NNBSP.to_owned(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(pattern: &str) -> ClockFormat {
        parse_short_time(pattern, "AM".to_owned(), "PM".to_owned())
    }

    #[test]
    fn short_time_patterns() {
        let c = clock("HH:mm");
        assert!(c.hour24 && c.leading_zero && !c.designator_first);
        assert_eq!(c.separator, ":");

        let c = clock("h:mm tt");
        assert!(!c.hour24 && !c.leading_zero && !c.designator_first);
        assert_eq!(c.separator, ":");

        let c = clock("tt h:mm");
        assert!(!c.hour24 && c.designator_first);

        let c = clock("H.mm");
        assert!(c.hour24 && !c.leading_zero);
        assert_eq!(c.separator, ".");

        let c = clock("HH 'h' mm");
        assert_eq!(c.separator, " h ");

        let c = clock("hh:mm");
        assert!(!c.hour24 && c.leading_zero);

        let c = clock("garbage");
        assert!(c.hour24 && c.leading_zero);
        assert_eq!((c.am.as_str(), c.pm.as_str()), ("AM", "PM"));
    }

    #[test]
    fn day_month_order_skips_weekday_names_and_quotes() {
        assert_eq!(day_month_order("d MMMM"), Some(DayMonthOrder::DayMonth));
        assert_eq!(day_month_order("MMMM d"), Some(DayMonthOrder::MonthDay));
        assert_eq!(
            day_month_order("dddd, MMMM d, yyyy"),
            Some(DayMonthOrder::MonthDay)
        );
        assert_eq!(
            day_month_order("'den 'd MMMM yyyy"),
            Some(DayMonthOrder::DayMonth)
        );
        assert_eq!(day_month_order("d. MMMM"), Some(DayMonthOrder::DayMonth));
        assert_eq!(day_month_order("M'月'd'日'"), Some(DayMonthOrder::MonthDay));
        assert_eq!(day_month_order("yyyy-MM-dd"), Some(DayMonthOrder::MonthDay));
        assert_eq!(day_month_order("yyyy"), None);
    }

    #[test]
    fn percent_and_separators() {
        assert_eq!(
            percent_pattern("0"),
            Some(PercentPattern::NumberSpacePercent)
        );
        assert_eq!(percent_pattern("1"), Some(PercentPattern::NumberPercent));
        assert_eq!(percent_pattern("2"), Some(PercentPattern::PercentNumber));
        assert_eq!(
            percent_pattern("3"),
            Some(PercentPattern::PercentSpaceNumber)
        );
        assert_eq!(percent_pattern("9"), None);
        assert_eq!(group_separator("\u{00A0}"), "\u{202F}");
        assert_eq!(group_separator(" "), "\u{202F}");
        assert_eq!(group_separator(","), ",");
        assert_eq!(group_separator("."), ".");
    }

    #[test]
    fn reads_a_complete_format_from_windows() {
        let f = read(TimeZone::UTC);
        assert!(!f.name.is_empty());
        assert!(f.weekdays_abbrev.iter().all(|s| !s.is_empty()));
        assert!(f.weekdays_full.iter().all(|s| !s.is_empty()));
        assert!(f.months_abbrev.iter().all(|s| !s.is_empty()));
        assert!(!f.clock.separator.is_empty());
        assert!(!f.decimal_separator.is_empty());
        assert_eq!(f.tz, TimeZone::UTC);
    }
}
