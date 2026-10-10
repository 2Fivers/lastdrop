//! The tray icon model and its per-size geometry (R§9).

use serde::{Deserialize, Serialize};

use super::bar::{BarInput, bar_input};
use super::build::BuildContext;
use crate::color::ColorRole;
use crate::grammar::a11y::tray_tooltip;
use crate::grammar::cells::pct;
use crate::stats::{Evaluation, RowState};

/// The tray shows one or two digits, never `100` (R§9.1: EXHAUSTED has its own icon).
const TRAY_MAX_PCT: u8 = 99;

/// The icon's shape ladder (R§9.1), for the winner (R§3.10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrayIcon {
    /// OK: the winner's `pct` (one or two digits, never `100`) over a mini pace bar (fill
    /// TextPrimary, track, tick at `e`; no tick when `e` is none, e.g. IDLE `0`). STALE (incl.
    /// PAUSED, SIGN_IN): digits and dashed bar in the stale colour, tick frozen.
    Digits {
        /// `pct` as text, one or two digits.
        digits: String,
        /// TextPrimary, or Stale.
        role: ColorRole,
        /// The mini bar.
        bar: BarInput,
    },
    /// WARNING (incl. countdown 31–59 min): outlined tile in Caution, digits TextPrimary centred,
    /// no bar. Balance winners: digits = `pct` of the top-up spent.
    CautionTile {
        /// `pct` as text.
        digits: String,
    },
    /// CRITICAL (level or runway): filled tile in Critical, digits TextOnAccent.
    CriticalTile {
        /// `pct` as text.
        digits: String,
    },
    /// EXHAUSTED: filled tile in Critical, hollow drop in TextOnAccent.
    ExhaustedTile,
    /// LOADING / no services: the Lastdrop drop, filled, TextPrimary.
    Drop,
}

/// What the tray icon shows (R§9). Re-rendered on theme, accent, high contrast or DPI change;
/// `szTip` updated with every change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrayModel {
    /// The icon.
    pub icon: TrayIcon,
    /// `szTip` (≤ 127 characters), also the accessible name (R§9.3).
    pub tooltip: String,
}

/// The hand-tuned layout of one icon size, device px, origin top-left (R§9.2 table).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayLayout {
    /// Layout key: 16, 20, 24 or 32.
    pub key: u32,
    /// Digits (OK, STALE): font size px, weight 600.
    pub digits_font: f32,
    /// Letter spacing after every digit (`IDWriteTextLayout1::SetCharacterSpacing`).
    pub letter_spacing: f32,
    /// Baseline y of the digits over the bar.
    pub digits_baseline: f32,
    /// Bar x.
    pub bar_x: i32,
    /// Bar length.
    pub bar_len: i32,
    /// Track y.
    pub track_y: i32,
    /// Track height (`hair`).
    pub track_h: i32,
    /// Fill y.
    pub fill_y: i32,
    /// Fill height.
    pub fill_h: i32,
    /// Tick y.
    pub tick_y: i32,
    /// Tick height.
    pub tick_h: i32,
    /// Tick width.
    pub tick_w: i32,
    /// Gap either side of the tick.
    pub gap: i32,
    /// Tile corner radius.
    pub tile_radius: f32,
    /// Tile outline width (stroke centred on a rect inset by half the stroke).
    pub tile_outline: f32,
    /// Digits on a tile: font size px, weight 600.
    pub tile_digits_font: f32,
    /// Digits on a tile: baseline y.
    pub tile_digits_baseline: f32,
    /// Hollow drop stroke in the EXHAUSTED tile.
    pub drop_stroke: f32,
}

/// The four layouts of R§9.2, smallest first.
pub const TRAY_LAYOUTS: [TrayLayout; 4] = [
    TrayLayout {
        key: 16,
        digits_font: 11.0,
        letter_spacing: -0.4,
        digits_baseline: 9.0,
        bar_x: 1,
        bar_len: 14,
        track_y: 13,
        track_h: 1,
        fill_y: 12,
        fill_h: 3,
        tick_y: 11,
        tick_h: 5,
        tick_w: 1,
        gap: 1,
        tile_radius: 3.0,
        tile_outline: 1.0,
        tile_digits_font: 11.0,
        tile_digits_baseline: 12.0,
        drop_stroke: 1.25,
    },
    TrayLayout {
        key: 20,
        digits_font: 13.0,
        letter_spacing: -0.4,
        digits_baseline: 11.0,
        bar_x: 1,
        bar_len: 18,
        track_y: 16,
        track_h: 1,
        fill_y: 15,
        fill_h: 3,
        tick_y: 13,
        tick_h: 7,
        tick_w: 1,
        gap: 1,
        tile_radius: 4.0,
        tile_outline: 1.0,
        tile_digits_font: 13.0,
        tile_digits_baseline: 15.0,
        drop_stroke: 1.25,
    },
    TrayLayout {
        key: 24,
        digits_font: 16.0,
        letter_spacing: -0.5,
        digits_baseline: 13.0,
        bar_x: 1,
        bar_len: 22,
        track_y: 19,
        track_h: 1,
        fill_y: 18,
        fill_h: 3,
        tick_y: 16,
        tick_h: 7,
        tick_w: 1,
        gap: 1,
        tile_radius: 4.0,
        tile_outline: 1.0,
        tile_digits_font: 16.0,
        tile_digits_baseline: 18.0,
        drop_stroke: 1.5,
    },
    TrayLayout {
        key: 32,
        digits_font: 21.0,
        letter_spacing: -0.6,
        digits_baseline: 17.0,
        bar_x: 2,
        bar_len: 28,
        track_y: 25,
        track_h: 2,
        fill_y: 24,
        fill_h: 4,
        tick_y: 21,
        tick_h: 10,
        tick_w: 2,
        gap: 2,
        tile_radius: 6.0,
        tile_outline: 2.0,
        tile_digits_font: 21.0,
        tile_digits_baseline: 24.0,
        drop_stroke: 2.0,
    },
];

/// The layout for an icon of `size` px (`GetSystemMetricsForDpi(SM_CXSMICON)`) and the offset
/// that centres it (R§9.1): sizes between the keys use the next smaller layout, centred,
/// offset `floor((size − key) / 2)` (28 → the 24 layout with 2 px margin; 36, 40 → 32). Never
/// scale a bitmap.
pub fn tray_layout_for(size: u32) -> (&'static TrayLayout, u32) {
    let layout = TRAY_LAYOUTS
        .iter()
        .rev()
        .find(|l| l.key <= size)
        .unwrap_or(&TRAY_LAYOUTS[0]);
    (layout, size.saturating_sub(layout.key) / 2)
}

/// The tray model for the winner (R§9.1, R§9.3). Over the whole evaluation, regardless of what
/// the readout shows.
pub fn build_tray(eval: &Evaluation, cx: &BuildContext<'_>) -> TrayModel {
    let tooltip = tray_tooltip(eval, &cx.text);
    let Some((_, a)) = eval.winner.as_ref().and_then(|k| eval.assessment(k)) else {
        return TrayModel {
            icon: TrayIcon::Drop,
            tooltip,
        };
    };
    let digits = pct(a).min(TRAY_MAX_PCT).to_string();
    let icon = match a.state {
        RowState::Loading => TrayIcon::Drop,
        RowState::Exhausted => TrayIcon::ExhaustedTile,
        RowState::CriticalLevel | RowState::CriticalRunway => TrayIcon::CriticalTile { digits },
        RowState::Warning => TrayIcon::CautionTile { digits },
        RowState::Stale(_) => TrayIcon::Digits {
            digits,
            role: ColorRole::Stale,
            bar: bar_input(a, ColorRole::TextPrimary),
        },
        RowState::Ok
        | RowState::OkNoReset
        | RowState::Idle
        | RowState::OkBalance
        | RowState::JustReset => TrayIcon::Digits {
            digits,
            role: ColorRole::TextPrimary,
            bar: bar_input(a, ColorRole::TextPrimary),
        },
    };
    TrayModel { icon, tooltip }
}

#[cfg(test)]
mod tests {
    use super::tray_layout_for;

    #[test]
    fn in_between_sizes_use_the_next_smaller_layout_centred() {
        assert_eq!(tray_layout_for(16).0.key, 16);
        assert_eq!(tray_layout_for(28).0.key, 24);
        assert_eq!(tray_layout_for(28).1, 2);
        assert_eq!(tray_layout_for(36).0.key, 32);
        assert_eq!(tray_layout_for(40).1, 4);
    }
}
