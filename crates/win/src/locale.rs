//! The user's regional format for the text grammar (R§5.2, R§5.6): `GetLocaleInfoEx(
//! LOCALE_NAME_USER_DEFAULT, …)` for `LOCALE_SSHORTTIME`, `LOCALE_S1159`/`LOCALE_S2359`,
//! `LOCALE_SABBREVDAYNAME1..7`, `LOCALE_SDAYNAME1..7`, `LOCALE_SABBREVMONTHNAME1..12`,
//! `LOCALE_IPOSITIVEPERCENT`, `LOCALE_STHOUSAND`, `LOCALE_SDECIMAL`, `LOCALE_SSHORTDATE` (day/month
//! order). Re-read on `WM_SETTINGCHANGE` "intl".
//!
//! Threading: main thread.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use lastdrop_core::grammar::LocaleFormat;
use lastdrop_core::jiff::tz::TimeZone;

/// Reads the user's regional format; `tz` is the system time zone (the app reads it with
/// `TimeZone::system()`).
pub fn read(tz: TimeZone) -> LocaleFormat {
    todo!("M1: GetLocaleInfoEx")
}
