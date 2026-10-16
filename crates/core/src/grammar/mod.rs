//! The text grammar: one grammar for every surface (R§5). Percent is always **used**, never
//! "left"; words appear only when something is wrong.
//!
//! - [`time`]: instants and durations (R§5.0), time points (R§5.2), countdowns (R§5.3), ages
//!   (R§5.4), card/flyout times (R§8.2).
//! - [`numbers`]: percent and amounts (R§5.6), balance durations (R§5.5).
//! - [`cells`]: readout right cells and compact cells by row state (R§5.7, R§5.8).
//! - [`a11y`]: UI Automation row sentences (R§11) and the tray tooltip (R§9.3).
//! - [`strings`]: every string as a resource (R§5.9, R Appendix B), with a [`Strings`] table
//!   per [`Language`] (English and Swedish).
//! - [`meters`]: meter names in the language of the strings (R§2 table).
//! - [`locale`]: the Windows regional format as data ([`LocaleFormat`]); `lastdrop-win` reads it
//!   with `GetLocaleInfoEx`.

pub mod a11y;
pub mod cells;
pub mod locale;
pub mod meters;
pub mod numbers;
pub mod strings;
pub mod time;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub use locale::LocaleFormat;
pub use strings::{English, Language, Msg, Strings, Swedish};

/// How time points are written in the readout (R§5.2; setting "Show times as").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeStyle {
    /// Clock mode (default): `14:56`, `Today`, `Mon`, `7d`.
    #[default]
    Clock,
    /// "Time remaining": `2h`, `11h`, `1d`, `45m`.
    Relative,
}

/// Everything a formatting function needs besides its value.
#[derive(Clone, Copy)]
pub struct TextContext<'a> {
    /// The clock (UTC). Shown as `floor_min(now)` (R§5.0).
    pub now: Timestamp,
    /// Clock or relative time points (R§5.2).
    pub time_style: TimeStyle,
    /// Regional format and time zone.
    pub locale: &'a LocaleFormat,
    /// The language's strings.
    pub strings: &'a dyn Strings,
}
