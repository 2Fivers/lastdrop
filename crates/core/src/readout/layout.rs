//! Readout widths (R§6.3), slot geometry (R§6.1) and the form choice (R§4.2), in device px.
//!
//! Widths are computed with the actual fonts at the actual DPI whenever the locale, DPI, pinned
//! set or enabled services change — **never** when data changes — so the readout keeps one width
//! in every state. Text is measured through [`TextMeasure`], implemented with DirectWrite in
//! `lastdrop-win` (and by table fakes in tests).
//!
//! Conversion rule (R§0, R§6.3): every logical constant becomes `round(c × s)` on its own, text
//! widths are measured at the em size `size × s` and ceiled, and a position is the **sum** of
//! converted parts (at 125 % the 6 px padding is 8 and the 40 px bar 50).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use serde::{Deserialize, Serialize};

use super::{ContentForm, ReadoutForm, TextStyle};

/// Measures text (implemented by `lastdrop-win` with DirectWrite).
pub trait TextMeasure {
    /// The advance width of `text` in **device px** at em size `style.size × scale`, not
    /// rounded; Segoe UI Variable with its default (tabular) figures (R§5.6).
    fn advance(&self, text: &str, style: TextStyle, scale: f64) -> f32;
}

/// Every string that can appear in a readout column, so the widths cover all of them (R§6.3).
/// Built by [`super::build_readout`] from the localized strings, the time style and the meters
/// that *can* appear.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidthInputs {
    /// Rows can come from more than one service: the 12 + 4 px glyph column is present (R§6.4).
    pub glyph_column: bool,
    /// Every label that can appear (`short_label`, with account tags in M2): `LW` (11 px font).
    pub labels: Vec<String>,
    /// `TIMEW`: every TIME token the shown windows can produce — `00:00` (tabular, the widest
    /// time), `Today`, the 7 weekday abbreviations, `88d`; relative tokens in relative mode.
    pub time_tokens: Vec<String>,
    /// From x = 30, regular: the REASON strings, `59m ago`, `47h ago`, `–`.
    pub statuses: Vec<String>,
    /// From x = 0, regular: `Reading…` and `Back ` + every TIME token (LOADING, stale EXHAUSTED).
    pub regular_at_zero: Vec<String>,
    /// From x = 0 after a 12 + 2 glyph, semibold: `Out ` + TIME tokens, the countdowns
    /// `59 min left`, `47 h left`, `29 d left`, `Back ` + TIME tokens, `Empty`.
    pub say_semibold: Vec<String>,
    /// `≈` and the balance durations `47h`, `13d`, `8w`, `9w+`; `None` when no balance can show.
    pub balance: Option<BalanceWidthInputs>,
    /// The compact cell, semibold after 14 px: `59 min`, `47 h`, `29 d`, `Empty`, `99%`.
    pub compact_semibold: Vec<String>,
    /// The compact cell, regular from x = 0: `99%`.
    pub compact_regular: Vec<String>,
}

/// Balance strings for `RIGHT_W` (R§6.3, last term).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BalanceWidthInputs {
    /// `≈`.
    pub approx: String,
    /// `47h`, `13d`, `8w`, `9w+`.
    pub durations: Vec<String>,
}

/// The readout's geometry in device px at one scale (R§6.1–R§6.3). Columns are x offsets from
/// the slot's left edge; rows are y offsets from the slot's top.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadoutLayout {
    /// `s = DPI / 96` of the taskbar's monitor.
    pub scale: f64,
    /// `slotH = round(40 s)`.
    pub slot_h: i32,
    /// `rowH = round(16 s)`.
    pub row_h: i32,
    /// `y1 = floor((slotH − 2 rowH) / 2)` (two rows).
    pub y_row1: i32,
    /// `y2 = y1 + rowH`.
    pub y_row2: i32,
    /// `floor((slotH − rowH) / 2)` (single row and compact; 12 at 100 %).
    pub y_single: i32,
    /// x of the service glyph (= padding), when the glyph column is present.
    pub x_glyph: Option<i32>,
    /// x of the label.
    pub x_label: i32,
    /// `LW`, device px.
    pub label_w: i32,
    /// x of the bar.
    pub x_bar: i32,
    /// `round(40 s)`.
    pub bar_w: i32,
    /// x of the right cell (and of the compact cell).
    pub x_right: i32,
    /// `RIGHT_W`, device px.
    pub right_w: i32,
    /// `COMPACT_W`, device px.
    pub compact_w: i32,
    /// `round(24 s)`: the VALUE box inside the right cell.
    pub value_box_w: i32,
    /// `round(30 s)`: the time column inside the right cell.
    pub time_x: i32,
    /// Slot width in the two-row / single-row forms:
    /// `6 + [12 + 4] + LW + 6 + 40 + 6 + RIGHT_W + 6` (converted per part).
    pub width_rows: i32,
    /// Slot width in the compact form: the same with `COMPACT_W`.
    pub width_compact: i32,
}

/// Computes R§6.3 at `scale`:
///
/// ```text
/// LW        = ceil(max width of the labels)                                  // 11 px font
/// TIMEW     = max width over the TIME tokens
/// RIGHT_W   = ceil(max( 30 + 11 + 1 + TIMEW,                                // VALUE + ↺TIME
///                       30 + max width(statuses),
///                       max width(regular_at_zero),
///                       12 + 2 + max semibold width(say_semibold),
///                       30 + width("≈") + 1 + max width(balance durations)  // only with a balance
///                     ))
/// COMPACT_W = ceil(max(width("99%"), 14 + semibold width(compact_semibold)))
/// width     = 6 + [12 + 4] + LW + 6 + 40 + 6 + (RIGHT_W | COMPACT_W) + 6
/// ```
///
/// Results at 100 %, en, Segoe UI Variable Small (R§6.3, R§13 "Widths"): 160 (one service),
/// 176 (several), 186 (a per-model label can appear), compact 134 / 150.
pub fn compute_layout(
    inputs: &WidthInputs,
    scale: f64,
    measure: &dyn TextMeasure,
) -> ReadoutLayout {
    todo!("M1: R§6.3")
}

/// The form to draw (R§4.2):
///
/// ```text
/// TWO_ROWS    content allows two rows, taskbar ≥ 40 px tall, room ≥ width(two rows)
/// SINGLE_ROW  only one meter configured/available
/// COMPACT     room < width(two rows) but ≥ width(compact), or taskbar < 40 px tall
/// HIDDEN      room < width(compact), or readout turned off (content Hidden)
/// ```
///
/// Going **up** a form needs `room ≥ width(form) + 8` (device: `round(8 s)`), so a flickering app
/// list cannot make it bounce; `previous` is the form currently drawn. `room` (device px) =
/// left edge of the overflow chevron (or the notification area) − 4 − (right edge of the
/// rightmost app button + 8), computed by the placement code. Full-screen apps and auto-hidden
/// taskbars are handled by placement before this is called.
pub fn choose_form(
    content: ContentForm,
    room: i32,
    taskbar_height: i32,
    layout: &ReadoutLayout,
    previous: ReadoutForm,
) -> ReadoutForm {
    todo!("M1: R§4.2")
}
