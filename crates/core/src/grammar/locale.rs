//! The Windows regional format as data (R§5.2, R§5.6, R§8.2).
//!
//! `lastdrop-win::locale::read()` fills it from `GetLocaleInfoEx(LOCALE_NAME_USER_DEFAULT, …)`
//! and the system time zone; tests use the fixed constructors below.

use jiff::tz::TimeZone;

/// Clock format from `LOCALE_SSHORTTIME` (R§5.2): pattern contains `H` → 24 h; `HH` vs `H`
/// decides the leading zero; `tt`/`t` is the AM/PM designator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockFormat {
    /// 24-hour clock.
    pub hour24: bool,
    /// Leading zero on the hour (`09:00` vs `9:00`).
    pub leading_zero: bool,
    /// Separator between hours and minutes.
    pub separator: String,
    /// AM designator (`LOCALE_SAM`), used only by the full short time (R§8.2).
    pub am: String,
    /// PM designator (`LOCALE_SPM`).
    pub pm: String,
    /// The designator comes before the time (`tt h:mm`, some East Asian locales).
    pub designator_first: bool,
}

/// Where the percent sign goes (`LOCALE_IPOSITIVEPERCENT`, R§5.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PercentPattern {
    /// `42%` (en).
    NumberPercent,
    /// `42 %` with U+202F NARROW NO-BREAK SPACE (sv-SE).
    NumberSpacePercent,
    /// `%42`.
    PercentNumber,
    /// `% 42`.
    PercentSpaceNumber,
}

/// Order of day and month in short dates without a year (`Sun 1 Nov` vs `Sun Nov 1`, R§8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayMonthOrder {
    /// `1 Nov`.
    DayMonth,
    /// `Nov 1`.
    MonthDay,
}

/// The user's regional format and time zone (R§0 "Times are kept in UTC and formatted in the
/// user's local time zone and Windows regional format").
#[derive(Clone, Debug, PartialEq)]
pub struct LocaleFormat {
    /// BCP 47 name (`en-GB`), for logs and string-table selection.
    pub name: String,
    /// The local time zone.
    pub tz: TimeZone,
    /// Clock format.
    pub clock: ClockFormat,
    /// Abbreviated weekday names, Monday first (`LOCALE_SABBREVDAYNAME1..7`; sv-SE lower-case
    /// `mån`, kept as is, R§5.9).
    pub weekdays_abbrev: [String; 7],
    /// Full weekday names, Monday first (`LOCALE_SDAYNAME1..7`; UIA sentences, R§11).
    pub weekdays_full: [String; 7],
    /// Abbreviated month names, January first (`LOCALE_SABBREVMONTHNAME1..12`).
    pub months_abbrev: [String; 12],
    /// Day/month order in short dates.
    pub day_month: DayMonthOrder,
    /// Percent sign placement.
    pub percent: PercentPattern,
    /// Digit grouping separator (`LOCALE_STHOUSAND`: `,` en; U+202F sv-SE, R§5.6).
    pub group_separator: String,
    /// Decimal separator (`LOCALE_SDECIMAL`).
    pub decimal_separator: String,
}

fn strings<const N: usize>(items: [&str; N]) -> [String; N] {
    items.map(str::to_owned)
}

impl LocaleFormat {
    /// en-GB: 24 h with leading zero (`09:00`), `1 Nov`, `42%`, `1,240`. The default locale of the
    /// R§13 test vectors ("24 h locale unless noted").
    pub fn en_gb(tz: TimeZone) -> LocaleFormat {
        LocaleFormat {
            name: "en-GB".to_owned(),
            tz,
            clock: ClockFormat {
                hour24: true,
                leading_zero: true,
                separator: ":".to_owned(),
                am: "am".to_owned(),
                pm: "pm".to_owned(),
                designator_first: false,
            },
            weekdays_abbrev: strings(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]),
            weekdays_full: strings([
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
                "Sunday",
            ]),
            months_abbrev: strings([
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ]),
            day_month: DayMonthOrder::DayMonth,
            percent: PercentPattern::NumberPercent,
            group_separator: ",".to_owned(),
            decimal_separator: ".".to_owned(),
        }
    }

    /// en-US: 12 h without leading zero (`2:56 PM`), `Nov 1`, `42%`, `1,240`. The "12-h locale"
    /// column of R§13.
    pub fn en_us(tz: TimeZone) -> LocaleFormat {
        LocaleFormat {
            name: "en-US".to_owned(),
            clock: ClockFormat {
                hour24: false,
                leading_zero: false,
                separator: ":".to_owned(),
                am: "AM".to_owned(),
                pm: "PM".to_owned(),
                designator_first: false,
            },
            day_month: DayMonthOrder::MonthDay,
            ..LocaleFormat::en_gb(tz)
        }
    }

    /// sv-SE: 24 h (`09:00`), lower-case weekdays (`mån`), `42 %` and `1 240` with U+202F
    /// (R§5.2, R§5.6). The words come from the language's table, not from here: the regional
    /// format and the language are separate Windows settings.
    pub fn sv_se(tz: TimeZone) -> LocaleFormat {
        LocaleFormat {
            name: "sv-SE".to_owned(),
            tz,
            clock: ClockFormat {
                hour24: true,
                leading_zero: true,
                separator: ":".to_owned(),
                am: "fm".to_owned(),
                pm: "em".to_owned(),
                designator_first: false,
            },
            weekdays_abbrev: strings(["mån", "tis", "ons", "tors", "fre", "lör", "sön"]),
            weekdays_full: strings([
                "måndag", "tisdag", "onsdag", "torsdag", "fredag", "lördag", "söndag",
            ]),
            months_abbrev: strings([
                "jan", "feb", "mars", "apr", "maj", "juni", "juli", "aug", "sep", "okt", "nov",
                "dec",
            ]),
            day_month: DayMonthOrder::DayMonth,
            percent: PercentPattern::NumberSpacePercent,
            group_separator: "\u{202F}".to_owned(),
            decimal_separator: ",".to_owned(),
        }
    }
}
