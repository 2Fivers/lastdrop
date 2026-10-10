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

use serde::{Deserialize, Serialize};

use super::{ContentForm, ReadoutForm, TextStyle};
use crate::consts::layout::{
    BALANCE_APPROX_GAP, BAR_W, COLUMN_GAP, COMPACT_GLYPH_ADVANCE, FORM_UP_HYSTERESIS,
    MIN_TWO_ROW_TASKBAR_H, RESET_GLYPH, RESET_GLYPH_GAP, ROW_H, SERVICE_GLYPH, SERVICE_GLYPH_GAP,
    SEVERITY_GLYPH, SEVERITY_GLYPH_GAP, SLOT_H, SLOT_PAD_X, TIME_X, VALUE_BOX_W,
};
use crate::util::round_i32;

/// A logical constant in device px: `round(c × s)`, on its own (R§0).
fn px(c: f32, scale: f64) -> i32 {
    round_i32(f64::from(c) * scale)
}

/// The widest of `texts` in `style`, device px, each width ceiled (R§6.3 "measured … and
/// ceiled"); 0 for none.
fn widest(texts: &[String], style: TextStyle, scale: f64, measure: &dyn TextMeasure) -> i32 {
    texts
        .iter()
        .map(|t| {
            let w = f64::from(measure.advance(t, style, scale));
            // Text widths are small and finite; `as` saturates otherwise.
            if w.is_finite() { w.ceil() as i32 } else { 0 }
        })
        .max()
        .unwrap_or(0)
        .max(0)
}

/// The rank of a form for the "going up needs + 8" rule (R§4.2).
fn form_level(form: ReadoutForm) -> u8 {
    match form {
        ReadoutForm::Hidden => 0,
        ReadoutForm::Compact => 1,
        ReadoutForm::SingleRow | ReadoutForm::TwoRows => 2,
    }
}

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
    let c = |v: f32| px(v, scale);
    let w = |texts: &[String], style: TextStyle| widest(texts, style, scale, measure);

    let label_w = w(&inputs.labels, TextStyle::LABEL);
    let time_w = w(&inputs.time_tokens, TextStyle::ROW);
    let mut right_w = (c(TIME_X) + c(RESET_GLYPH) + c(RESET_GLYPH_GAP) + time_w)
        .max(c(TIME_X) + w(&inputs.statuses, TextStyle::ROW))
        .max(w(&inputs.regular_at_zero, TextStyle::ROW))
        .max(
            c(SEVERITY_GLYPH)
                + c(SEVERITY_GLYPH_GAP)
                + w(&inputs.say_semibold, TextStyle::ROW_SEMIBOLD),
        );
    if let Some(balance) = &inputs.balance {
        let approx = w(std::slice::from_ref(&balance.approx), TextStyle::ROW);
        right_w = right_w.max(
            c(TIME_X) + approx + c(BALANCE_APPROX_GAP) + w(&balance.durations, TextStyle::ROW),
        );
    }
    let compact_w = w(&inputs.compact_regular, TextStyle::ROW)
        .max(c(COMPACT_GLYPH_ADVANCE) + w(&inputs.compact_semibold, TextStyle::ROW_SEMIBOLD));

    let pad = c(SLOT_PAD_X);
    let x_glyph = inputs.glyph_column.then_some(pad);
    let x_label = pad
        + if inputs.glyph_column {
            c(SERVICE_GLYPH) + c(SERVICE_GLYPH_GAP)
        } else {
            0
        };
    let x_bar = x_label + label_w + c(COLUMN_GAP);
    let bar_w = c(BAR_W);
    let x_right = x_bar + bar_w + c(COLUMN_GAP);

    let slot_h = c(SLOT_H);
    let row_h = c(ROW_H);
    let y_row1 = (slot_h - 2 * row_h).div_euclid(2);
    ReadoutLayout {
        scale,
        slot_h,
        row_h,
        y_row1,
        y_row2: y_row1 + row_h,
        y_single: (slot_h - row_h).div_euclid(2),
        x_glyph,
        x_label,
        label_w,
        x_bar,
        bar_w,
        x_right,
        right_w,
        compact_w,
        value_box_w: c(VALUE_BOX_W),
        time_x: c(TIME_X),
        width_rows: x_right + right_w + pad,
        width_compact: x_right + compact_w + pad,
    }
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
    let rows_form = match content {
        ContentForm::Hidden => return ReadoutForm::Hidden,
        ContentForm::TwoRows => ReadoutForm::TwoRows,
        ContentForm::SingleRow => ReadoutForm::SingleRow,
    };
    let hysteresis = px(FORM_UP_HYSTERESIS, layout.scale);
    let fits = |form: ReadoutForm, width: i32| {
        let up = form_level(form) > form_level(previous);
        room >= width + if up { hysteresis } else { 0 }
    };
    let tall = taskbar_height >= px(MIN_TWO_ROW_TASKBAR_H, layout.scale);
    if tall && fits(rows_form, layout.width_rows) {
        rows_form
    } else if fits(ReadoutForm::Compact, layout.width_compact) {
        ReadoutForm::Compact
    } else {
        ReadoutForm::Hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readout::FontWeight;

    /// Fixed advances: 6 px per character at 12 px regular, 7 semibold, 5.5 for labels; scaled.
    struct Fake;

    impl TextMeasure for Fake {
        fn advance(&self, text: &str, style: TextStyle, scale: f64) -> f32 {
            let per = match (style.weight, style.size < 12.0) {
                (_, true) => 5.5,
                (FontWeight::Regular, false) => 6.0,
                (FontWeight::Semibold, false) => 7.0,
            };
            (text.chars().count() as f64 * per * scale) as f32
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn inputs(glyph_column: bool) -> WidthInputs {
        WidthInputs {
            glyph_column,
            labels: strings(&["5h", "Wk"]),
            time_tokens: strings(&["00:00", "Today"]),
            statuses: strings(&["59m ago"]),
            regular_at_zero: strings(&["Reading…"]),
            say_semibold: strings(&["59 min left"]),
            balance: None,
            compact_semibold: strings(&["59 min"]),
            compact_regular: strings(&["99%"]),
        }
    }

    #[test]
    fn widths_are_sums_of_converted_parts() {
        let l = compute_layout(&inputs(false), 1.0, &Fake);
        // LW = ceil(2 × 5.5) = 11; RIGHT_W = max(30 + 12 + 30, 30 + 42, 48, 14 + 77) = 91;
        // COMPACT_W = max(18, 14 + 42) = 56.
        assert_eq!((l.label_w, l.right_w, l.compact_w), (11, 91, 56));
        assert_eq!(l.x_glyph, None);
        assert_eq!((l.x_label, l.x_bar, l.x_right), (6, 23, 69));
        assert_eq!((l.width_rows, l.width_compact), (166, 131));
        assert_eq!(
            (l.slot_h, l.row_h, l.y_row1, l.y_row2, l.y_single),
            (40, 16, 4, 20, 12)
        );
        let g = compute_layout(&inputs(true), 1.0, &Fake);
        assert_eq!(g.x_glyph, Some(6));
        assert_eq!(g.width_rows, l.width_rows + 16);

        let big = compute_layout(&inputs(true), 1.25, &Fake);
        assert_eq!((big.x_glyph, big.x_label, big.bar_w), (Some(8), 28, 50));
        assert_eq!(
            big.width_rows,
            8 + 15 + 5 + big.label_w + 8 + 50 + 8 + big.right_w + 8
        );
        assert_eq!((big.value_box_w, big.time_x), (30, 38));
        assert_eq!((big.slot_h, big.row_h, big.y_row1), (50, 20, 5));
    }

    #[test]
    fn balance_strings_widen_the_right_cell() {
        let mut i = inputs(false);
        i.balance = Some(BalanceWidthInputs {
            approx: "≈".to_owned(),
            durations: strings(&["9w+ and more text"]),
        });
        let l = compute_layout(&i, 1.0, &Fake);
        assert_eq!(l.right_w, 30 + 6 + 1 + 102);
    }

    #[test]
    fn forms_step_down_and_need_room_plus_8_to_step_up() {
        let l = compute_layout(&inputs(false), 1.0, &Fake);
        let (rows, compact) = (l.width_rows, l.width_compact);
        let two = ContentForm::TwoRows;
        let choose = |room, h, prev| choose_form(two, room, h, &l, prev);
        assert_eq!(choose(rows, 48, ReadoutForm::TwoRows), ReadoutForm::TwoRows);
        assert_eq!(
            choose(rows - 1, 48, ReadoutForm::TwoRows),
            ReadoutForm::Compact
        );
        assert_eq!(choose(rows, 48, ReadoutForm::Compact), ReadoutForm::Compact);
        assert_eq!(
            choose(rows + 8, 48, ReadoutForm::Compact),
            ReadoutForm::TwoRows
        );
        assert_eq!(
            choose(compact, 48, ReadoutForm::Compact),
            ReadoutForm::Compact
        );
        assert_eq!(
            choose(compact - 1, 48, ReadoutForm::Compact),
            ReadoutForm::Hidden
        );
        assert_eq!(
            choose(compact + 7, 48, ReadoutForm::Hidden),
            ReadoutForm::Hidden
        );
        assert_eq!(
            choose(compact + 8, 48, ReadoutForm::Hidden),
            ReadoutForm::Compact
        );
        // A short taskbar gets the compact form whatever the room.
        assert_eq!(choose(1000, 32, ReadoutForm::TwoRows), ReadoutForm::Compact);
        assert_eq!(
            choose_form(ContentForm::SingleRow, 1000, 48, &l, ReadoutForm::Hidden),
            ReadoutForm::SingleRow
        );
        assert_eq!(
            choose_form(ContentForm::Hidden, 1000, 48, &l, ReadoutForm::TwoRows),
            ReadoutForm::Hidden
        );
    }
}
