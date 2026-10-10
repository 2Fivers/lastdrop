//! The pace bar: the normative pixel algorithm (R§6.6), shared by the readout (W 40, rowH 16),
//! the hover card (296, 11), the flyout (310, 9) and, with fixed tables, the tray icon (R§9.2).
//!
//! The model carries a [`BarInput`]; each surface calls [`bar_geometry`] at its own scale and
//! paints the returned rectangles. Gaps are **not painted** (nothing is drawn there), so the bar
//! works on any backdrop.

use serde::{Deserialize, Serialize};

use super::tray::TrayLayout;
use crate::color::ColorRole;
use crate::consts::TOL_PACE;
use crate::stats::{AssessedMeter, Assessment, RowState};
use crate::util::{round_i32, sane_share};

/// `odd(x) = 2 × round((x − hair) / 2) + hair` (R§6.6): keeps strokes centred on the track.
fn odd(x: f64, hair: i32) -> i32 {
    2 * round_i32((x - f64::from(hair)) / 2.0) + hair
}

/// `[x0, x1)` minus the cuts, empty pieces dropped (the reference drawing code's `minus`).
fn minus(x0: i32, x1: i32, cuts: &[Seg]) -> Vec<Seg> {
    if x1 <= x0 {
        return Vec::new();
    }
    let mut parts = vec![Seg { x0, x1 }];
    for cut in cuts {
        parts = parts
            .into_iter()
            .flat_map(|p| {
                let left = Seg {
                    x0: p.x0,
                    x1: p.x1.min(cut.x0),
                };
                let right = Seg {
                    x0: p.x0.max(cut.x1),
                    x1: p.x1,
                };
                [left, right]
            })
            .filter(|s| s.x1 > s.x0)
            .collect();
    }
    parts
}

/// Splits fill segments into square dashes: pixel column `x` is painted iff
/// `(x mod 3·hair) < 2·hair`, one phase from x = 0 for every segment (R§6.6).
fn dashes(fill: &[Seg], hair: i32) -> Vec<Seg> {
    let period = 3 * hair;
    let mut out = Vec::new();
    for seg in fill {
        let mut x = seg.x0 - seg.x0.rem_euclid(period);
        while x < seg.x1 {
            let (a, b) = (x.max(seg.x0), (x + 2 * hair).min(seg.x1));
            if b > a {
                out.push(Seg { x0: a, x1: b });
            }
            x += period;
        }
    }
    out
}

/// The fill end `f` and tick `k` of R§6.6 for a bar `len` pixels long with stroke `hair`.
fn fill_and_tick(input: &BarInput, len: i32, hair: i32) -> (i32, Option<i32>) {
    let u = sane_share(input.used);
    let mut f = round_i32(u.min(1.0) * f64::from(len));
    if u > 0.0 && f == 0 {
        f = 1;
    }
    match input.mode {
        BarMode::Exhausted => f = len,
        BarMode::Loading => f = 0,
        BarMode::Normal => {}
    }
    let tick = match (input.elapsed, input.mode) {
        (Some(e), BarMode::Normal | BarMode::Exhausted) if e.is_finite() => {
            Some(round_i32(e * f64::from(len)).clamp(0, (len - hair).max(0)))
        }
        _ => None,
    };
    if let (Some(k), Some(e), BarMode::Normal) = (tick, input.elapsed, input.mode) {
        if u < e - TOL_PACE && f > k - hair - 1 {
            f = (k - hair - 1).max(i32::from(u > 0.0));
        }
        if u > e + TOL_PACE && f < k + 2 * hair + 1 {
            f = len.min(k + 2 * hair + 1);
        }
    }
    (f, tick)
}

/// The two gaps either side of the tick, `[k − hair, k)` and `[k + hair, k + 2·hair)`.
fn gaps(tick: Option<i32>, hair: i32) -> Vec<Seg> {
    match tick {
        Some(k) => vec![
            Seg {
                x0: k - hair,
                x1: k,
            },
            Seg {
                x0: k + hair,
                x1: k + 2 * hair,
            },
        ],
        None => Vec::new(),
    }
}

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
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let len = round_i32(f64::from(width) * s).max(0);
    // `floor` of a positive finite scale fits easily in an i32.
    let hair = (s.floor() as i32).max(1);
    let fill_h = odd(3.0 * s, hair);
    let tick_h = odd(9.0 * s, hair);
    let height = round_i32(f64::from(row_h) * s);
    let y_track = (height - hair).div_euclid(2);
    let y_fill = y_track - (fill_h - hair) / 2;
    let y_tick = y_track - (tick_h - hair) / 2;

    let (f, tick) = fill_and_tick(input, len, hair);
    let cuts = gaps(tick, hair);
    let track = minus(f, len, &cuts);
    let solid = minus(0, f, &cuts);
    let (fill, round_start, round_end) = if input.dashed {
        (dashes(&solid, hair), false, false)
    } else {
        let start = solid.first().is_some_and(|s| s.x0 == 0);
        let end = solid.last().is_some_and(|s| s.x1 == f);
        (solid, start, end)
    };
    BarGeometry {
        len,
        height,
        hair,
        fill_h,
        tick_h,
        y_track,
        y_fill,
        y_tick,
        fill_end: f,
        tick,
        track,
        fill,
        round_start,
        round_end,
    }
}

/// The bar inputs of an assessed meter (R§5.7 "Bar" column, R§6.6 colours): `u`, `e` (none for
/// balances, LOADING, IDLE, no `R`/`L`), the mode (EXHAUSTED full length, LOADING track only),
/// dashes and the stale colour when the service is stale, the fill role by state with `calm` for
/// OK rows — TextPrimary on the taskbar and in the hover card, AccentFill in the flyout.
pub fn bar_input(a: &Assessment, calm: ColorRole) -> BarInput {
    let (used, elapsed, exhausted) = match &a.meter {
        AssessedMeter::Window(w) => (w.used, w.elapsed, w.exhausted),
        AssessedMeter::Balance(b) => (b.stats.used_share, None, b.stats.exhausted),
        AssessedMeter::Loading => (0.0, None, false),
    };
    let stale = matches!(a.state, RowState::Stale(_));
    let mode = match a.state {
        RowState::Loading => BarMode::Loading,
        RowState::Exhausted => BarMode::Exhausted,
        // A stale reading that was used up is drawn full length, dashed.
        RowState::Stale(_) if exhausted => BarMode::Exhausted,
        _ => BarMode::Normal,
    };
    let fill = match a.state {
        RowState::Warning => ColorRole::Caution,
        RowState::CriticalLevel | RowState::CriticalRunway | RowState::Exhausted => {
            ColorRole::Critical
        }
        RowState::Stale(_) => ColorRole::Stale,
        RowState::Loading
        | RowState::Ok
        | RowState::OkNoReset
        | RowState::Idle
        | RowState::OkBalance
        | RowState::JustReset => calm,
    };
    BarInput {
        used,
        elapsed: if a.state == RowState::Loading {
            None
        } else {
            elapsed
        },
        mode,
        dashed: stale,
        fill,
        tick: if stale {
            ColorRole::Stale
        } else {
            ColorRole::TextPrimary
        },
        track: ColorRole::Track,
    }
}

/// The tray icon's mini bar (R§9.2): R§6.6's rules with the fixed values of one tray layout
/// (bar x and length, track/fill/tick rows and heights, `hair`, gap), no rounded ends, the same
/// stale dash phase. Segments are relative to the bar's x.
///
/// The rows are the table's absolute y values in the icon (`height` is the layout key), so a
/// renderer adds only the bar's x and the centring offset.
pub fn tray_bar_geometry(input: &BarInput, layout: &TrayLayout) -> BarGeometry {
    let len = layout.bar_len;
    let hair = layout.gap.max(1);
    let (f, tick) = fill_and_tick(input, len, hair);
    let cuts = gaps(tick, hair);
    let solid = minus(0, f, &cuts);
    BarGeometry {
        len,
        height: i32::try_from(layout.key).unwrap_or(i32::MAX),
        hair: layout.track_h,
        fill_h: layout.fill_h,
        tick_h: layout.tick_h,
        y_track: layout.track_y,
        y_fill: layout.fill_y,
        y_tick: layout.tick_y,
        fill_end: f,
        tick,
        track: minus(f, len, &cuts),
        fill: if input.dashed {
            dashes(&solid, hair)
        } else {
            solid
        },
        round_start: false,
        round_end: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readout::tray::TRAY_LAYOUTS;

    fn input(used: f64, elapsed: Option<f64>, mode: BarMode, dashed: bool) -> BarInput {
        BarInput {
            used,
            elapsed,
            mode,
            dashed,
            fill: ColorRole::TextPrimary,
            tick: ColorRole::TextPrimary,
            track: ColorRole::Track,
        }
    }

    fn segs(list: &[(i32, i32)]) -> Vec<Seg> {
        list.iter().map(|&(x0, x1)| Seg { x0, x1 }).collect()
    }

    #[test]
    fn odd_heights_and_rows_per_scale() {
        // (s, fillH, tickH, Hpx, yTrack) at rowH 16: R§6.6's 3, 3, 5, 5, 6 and 9, 11, 13, 15, 18.
        for (s, fill_h, tick_h) in [
            (1.0, 3, 9),
            (1.25, 3, 11),
            (1.5, 5, 13),
            (1.75, 5, 15),
            (2.0, 6, 18),
        ] {
            let g = bar_geometry(&input(0.5, None, BarMode::Normal, false), 40.0, 16.0, s);
            assert_eq!((g.fill_h, g.tick_h), (fill_h, tick_h), "s = {s}");
            assert_eq!(g.y_fill, g.y_track - (g.fill_h - g.hair) / 2);
        }
        let g = bar_geometry(&input(0.5, None, BarMode::Normal, false), 40.0, 16.0, 1.0);
        assert_eq!((g.height, g.y_track, g.y_fill, g.y_tick), (16, 7, 6, 3));
    }

    #[test]
    fn loading_is_track_only_and_rounded_ends_follow_the_fill() {
        let g = bar_geometry(
            &input(0.5, Some(0.5), BarMode::Loading, false),
            40.0,
            16.0,
            1.0,
        );
        assert!(g.fill.is_empty());
        assert_eq!(g.tick, None);
        assert_eq!(g.track, segs(&[(0, 40)]));
        let g = bar_geometry(
            &input(0.42, Some(0.5533), BarMode::Normal, false),
            40.0,
            16.0,
            1.0,
        );
        assert!(g.round_start && g.round_end);
        // The fill ends inside the left gap: no rounded end there.
        let g = bar_geometry(
            &input(0.55, Some(0.5533), BarMode::Normal, false),
            40.0,
            16.0,
            1.0,
        );
        assert!(g.round_start && !g.round_end);
    }

    #[test]
    fn stale_dashes_keep_one_phase_after_a_gap() {
        let g = bar_geometry(
            &input(0.68, Some(0.63), BarMode::Normal, true),
            40.0,
            16.0,
            1.0,
        );
        // Fill [0,24) [25,26) [27,28) → columns painted where x mod 3 < 2.
        assert_eq!(
            g.fill,
            segs(&[
                (0, 2),
                (3, 5),
                (6, 8),
                (9, 11),
                (12, 14),
                (15, 17),
                (18, 20),
                (21, 23),
                (25, 26),
                (27, 28),
            ])
        );
        assert!(!g.round_start && !g.round_end);
        // At 200 % the period is 6 with 4 painted.
        let g = bar_geometry(&input(0.2, None, BarMode::Normal, true), 40.0, 16.0, 2.0);
        assert_eq!(g.fill, segs(&[(0, 4), (6, 10), (12, 16)]));
    }

    #[test]
    fn tray_bars_use_the_fixed_tables() {
        let l16 = &TRAY_LAYOUTS[0];
        let g = tray_bar_geometry(&input(0.42, Some(0.5533), BarMode::Normal, false), l16);
        // 14 px: f = round(5.88) = 6, k = round(7.75) = 8; under pace: f ≤ k − 2 = 6.
        assert_eq!((g.len, g.fill_end, g.tick), (14, 6, Some(8)));
        assert_eq!(g.fill, segs(&[(0, 6)]));
        assert_eq!(g.track, segs(&[(6, 7), (8, 9), (10, 14)]));
        assert_eq!((g.y_track, g.y_fill, g.y_tick), (13, 12, 11));
        assert!(!g.round_start && !g.round_end);
        let l32 = &TRAY_LAYOUTS[3];
        let g = tray_bar_geometry(&input(0.9, Some(0.5), BarMode::Normal, false), l32);
        // 28 px, hair 2: f = 25, k = 14; ahead and already past k + 5.
        assert_eq!((g.fill_end, g.tick), (25, Some(14)));
        assert_eq!(g.fill, segs(&[(0, 12), (14, 16), (18, 25)]));
        assert_eq!(g.track, segs(&[(25, 28)]));
        let idle = tray_bar_geometry(&input(0.0, None, BarMode::Normal, false), l16);
        assert!(idle.fill.is_empty());
        assert_eq!(idle.tick, None);
    }
}
