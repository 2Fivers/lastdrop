//! The pace bar: the normative pixel algorithm (R§6.6), shared by the readout (W 40, rowH 16),
//! the hover card (296, 11), the flyout (310, 9) and, with fixed tables, the tray icon (R§9.2).
//!
//! The model carries a [`BarInput`]; each surface calls [`bar_geometry`] at its own scale and
//! paints the returned rectangles. Gaps are **not painted** (nothing is drawn there), so the bar
//! works on any backdrop.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use serde::{Deserialize, Serialize};

use super::tray::TrayLayout;
use crate::color::ColorRole;
use crate::stats::Assessment;

/// Special cases of the fill (R§6.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BarMode {
    /// `f = round(clamp(u, 0, 1) × Lpx)`, at least 1 when `u > 0`, with the pace correction.
    Normal,
    /// EXHAUSTED: `f = Lpx`, no pace correction.
    Exhausted,
    /// LOADING: `f = 0`, no tick.
    Loading,
}

/// What a bar shows, independent of DPI.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarInput {
    /// `u`, the used share (balances: the share of the top-up spent).
    pub used: f64,
    /// `e`, the elapsed share; `None` = no tick and no gaps (balance, LOADING, IDLE, `e` none).
    pub elapsed: Option<f64>,
    /// Special cases.
    pub mode: BarMode,
    /// STALE: the fill is square dashes (R§6.6).
    pub dashed: bool,
    /// Fill colour: TextPrimary on the taskbar and in the hover card, AccentFill in the flyout
    /// (calm); Caution; Critical; Stale.
    pub fill: ColorRole,
    /// Tick colour: TextPrimary (stale: Stale).
    pub tick: ColorRole,
    /// Track colour: Track (ControlStrongStrokeColorDefault).
    pub track: ColorRole,
}

/// A half-open span `[x0, x1)` of device pixels, relative to the bar's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Seg {
    /// First pixel column.
    pub x0: i32,
    /// One past the last pixel column.
    pub x1: i32,
}

/// The bar in device pixels (R§6.6). Rows are relative to the bar's row box (`Hpx` high).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BarGeometry {
    /// `Lpx = round(W × s)`.
    pub len: i32,
    /// `Hpx = round(rowH × s)`.
    pub height: i32,
    /// `hair = max(1, floor(s))`: track, tick and gap width.
    pub hair: i32,
    /// `fillH = odd(3 × s)`.
    pub fill_h: i32,
    /// `tickH = odd(9 × s)`.
    pub tick_h: i32,
    /// `yTrack = floor((Hpx − hair) / 2)`.
    pub y_track: i32,
    /// `yFill = yTrack − (fillH − hair) / 2`.
    pub y_fill: i32,
    /// `yTick = yTrack − (tickH − hair) / 2`.
    pub y_tick: i32,
    /// `f`, the fill end after corrections.
    pub fill_end: i32,
    /// `k`, the tick x; `None` without a tick.
    pub tick: Option<i32>,
    /// Track `[f, Lpx)` minus the gaps, height `hair` at `yTrack` (the segment under the tick
    /// column is covered by the tick).
    pub track: Vec<Seg>,
    /// Fill `[0, f)` minus the gaps, height `fillH` at `yFill`. When dashed: already split into
    /// dashes (pixel column `x` painted iff `(x mod 3·hair) < 2·hair`, one phase from x = 0, also
    /// after a gap).
    pub fill: Vec<Seg>,
    /// The fill's outer start (x = 0) is rounded with radius `fillH / 2` (not when dashed, not
    /// at the tray sizes).
    pub round_start: bool,
    /// The fill's outer end (x = f, unless it ends at a gap) is rounded.
    pub round_end: bool,
}

/// The normative pace bar algorithm (R§6.6), in device px:
///
/// ```text
/// Lpx    = round(W × s)
/// hair   = max(1, floor(s))                 // 1 px up to 199 %, 2 px at 200 %+
/// odd(x) = 2 × round((x − hair) / 2) + hair // keeps strokes centred on the track
/// fillH  = odd(3 × s)                       // 3, 3, 5, 5, 6 at 100/125/150/175/200 %
/// tickH  = odd(9 × s)                       // 9, 11, 13, 15, 18
/// Hpx    = round(rowH × s)
/// yTrack = floor((Hpx − hair) / 2);  yFill = yTrack − (fillH − hair)/2;  yTick = yTrack − (tickH − hair)/2
///
/// f = round(clamp(u,0,1) × Lpx);  if u > 0 && f == 0 { f = 1 };  EXHAUSTED: f = Lpx;  LOADING: f = 0
/// k = e ≠ none && !LOADING ? clamp(round(e × Lpx), 0, Lpx − hair) : none
/// if k and not EXHAUSTED:
///     if under (u < e − TOL_PACE) and f > k − hair − 1:   f = max(u > 0 ? 1 : 0, k − hair − 1)
///     if ahead (u > e + TOL_PACE) and f < k + 2·hair + 1: f = min(Lpx, k + 2·hair + 1)
/// gaps = [k − hair, k) and [k + hair, k + 2·hair)        // cut out, never painted
/// ```
///
/// `round` is half-up (R§0). Test vectors: R§13 "Pace bar geometry".
pub fn bar_geometry(input: &BarInput, width: f32, row_h: f32, scale: f64) -> BarGeometry {
    todo!("M1: R§6.6")
}

/// The bar inputs of an assessed meter (R§5.7 "Bar" column, R§6.6 colours): `u`, `e` (none for
/// balances, LOADING, IDLE, no `R`/`L`), the mode (EXHAUSTED full length, LOADING track only),
/// dashes and the stale colour when the service is stale, the fill role by state with `calm` for
/// OK rows — TextPrimary on the taskbar and in the hover card, AccentFill in the flyout.
pub fn bar_input(a: &Assessment, calm: ColorRole) -> BarInput {
    todo!("M1: R§5.7 bar column")
}

/// The tray icon's mini bar (R§9.2): R§6.6's rules with the fixed values of one tray layout
/// (bar x and length, track/fill/tick rows and heights, `hair`, gap), no rounded ends, the same
/// stale dash phase. Segments are relative to the bar's x.
pub fn tray_bar_geometry(input: &BarInput, layout: &TrayLayout) -> BarGeometry {
    todo!("M1: R§9.2 tray bar")
}
