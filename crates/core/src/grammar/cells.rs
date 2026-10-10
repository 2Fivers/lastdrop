//! Readout cell texts by row state (R§5.7) and the compact cell (R§5.8).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use super::TextContext;
use crate::readout::{CompactCell, RightCell};
use crate::stats::{Assessment, ServiceEval};

/// Where a row sits among the readout rows of its service. A stale service's rows take, in
/// order, REASON then AGE (R§5.7 "STALE status, per service"), so the cell depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RowPosition {
    /// 0-based index among the shown rows of this service.
    pub index_in_service: usize,
    /// How many rows of this service are shown.
    pub rows_in_service: usize,
}

/// The right cell of a row (R§5.7). `RIGHT_W` wide; sub-columns: the VALUE box (24 px,
/// right-aligned at x = 0) and time/status from x = 30; say-text from x = 0.
///
/// | State            | Right cell                                                                 |
/// |------------------|----------------------------------------------------------------------------|
/// | LOADING          | first row of the service: `Reading…` from x = 0 (TextSecondary); other rows: `–` in the VALUE box |
/// | OK               | `42%` + `↺14:56`                                                           |
/// | OK, no reset     | `42%` + `–`                                                                |
/// | IDLE             | `0%` + `–`                                                                 |
/// | OK, balance      | `62%` + `≈9d` (`–` without a forecast)                                     |
/// | CRITICAL_LEVEL   | `92%` **semibold** + `↺16:30`                                              |
/// | WARNING          | ▲ + `Out 15:20` (or `45 min left` when N ≤ 59)                             |
/// | CRITICAL_RUNWAY  | ● + countdown, always                                                      |
/// | EXHAUSTED        | ● + `Back 16:30` (no reset time, or balance: ● + `Empty`)                  |
/// | JUST_RESET       | `0%` + `↺7d` (the new `R'`)                                                |
/// | STALE            | `42%` (stale colour) + REASON or AGE                                       |
///
/// STALE details: a one-row service shows `Sign in` (SIGN_IN) or `Paused` (PAUSED), otherwise
/// AGE; a row whose frozen reading was EXHAUSTED shows `Back 16:30` from x = 0, regular weight,
/// stale colour, no glyph, and the status moves to the service's other row (one-row service: no
/// status). `Sign in` is AccentText; other statuses the stale colour.
pub fn right_cell(
    a: &Assessment,
    service: &ServiceEval,
    pos: RowPosition,
    cx: &TextContext<'_>,
) -> RightCell {
    todo!("M1: R§5.7")
}

/// The compact cell (R§5.8), left-aligned: OK `61%` · IDLE `0%` · STALE `61%` (stale colour) ·
/// WARNING ▲ `78%` semibold · any row that shows a countdown in the two-row form ▲/● + COUNTDOWN
/// without "left" (`12 min`, `6 h`) semibold · CRITICAL_LEVEL ● `92%` semibold · EXHAUSTED
/// ● `Empty` semibold · LOADING `–`.
pub fn compact_cell(a: &Assessment, cx: &TextContext<'_>) -> CompactCell {
    todo!("M1: R§5.8")
}
