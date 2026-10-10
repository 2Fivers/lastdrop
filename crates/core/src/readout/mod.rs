//! The taskbar readout, its hover card and the tray icon as models (R§4–R§9, R§11).
//!
//! A [`ReadoutModel`] is everything the Win32 readout renderer draws: rows with glyph ids,
//! label, bar *inputs*, text runs with colour roles (never colours), the compact cell, the
//! strings that define column widths, and the UI Automation names. The renderer (readout thread
//! in `lastdrop-win`) adds only device geometry, computed with this module's normative functions
//! at its own DPI: [`layout::compute_layout`] (R§6.3), [`layout::choose_form`] (R§4.2) and
//! [`bar::bar_geometry`] (R§6.6).
//!
//! - [`bar`]: the pace bar pixel algorithm (R§6.6) and its tray variant (R§9.2).
//! - [`layout`]: widths (R§6.3), slot geometry (R§6.1) and the form choice (R§4.2).
//! - [`hovercard`]: the hover card model (R§8).
//! - [`tray`]: the tray icon model and per-size geometry (R§9).
//! - [`build`]: builders from an [`crate::stats::Evaluation`].

pub mod bar;
pub mod build;
pub mod hovercard;
pub mod layout;
pub mod tray;

use serde::{Deserialize, Serialize};

pub use bar::{BarGeometry, BarInput, BarMode, Seg, bar_geometry, bar_input};
pub use build::{BuildContext, build_readout};
pub use hovercard::{HoverAnchor, HoverCardModel, build_hover_card};
pub use layout::{ReadoutLayout, TextMeasure, WidthInputs, choose_form, compute_layout};
pub use tray::{TrayIcon, TrayModel, build_tray};

use crate::color::ColorRole;
use crate::glyph::{Icon, ProviderGlyph, SeverityGlyph};
use crate::model::MeterKey;

/// A font family role (R§6.5, F§8). `lastdrop-win` and the Slint `Theme` map it to a family with
/// fallbacks (Segoe UI on Windows 10, Segoe MDL2 Assets for icons).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FontFace {
    /// Segoe UI Variable Small: readout rows, labels, card/flyout small text, tray digits.
    UiSmall,
    /// Segoe UI Variable Text: card titles, flyout names and values.
    UiText,
    /// Segoe UI Variable Display: the flyout's balance amount.
    UiDisplay,
    /// Segoe Fluent Icons.
    Icons,
}

/// Font weight (R§6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FontWeight {
    /// 400.
    Regular,
    /// 600.
    Semibold,
}

/// A text style: family role, size in logical px (em size `size × s` device px, not rounded,
/// R§0), weight.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// Family role.
    pub face: FontFace,
    /// Size in logical px; line height is 16 for every readout role (R§6.5).
    pub size: f32,
    /// Weight.
    pub weight: FontWeight,
}

impl TextStyle {
    /// Row text (value, time): Small 12, 400 (R§6.5).
    pub const ROW: TextStyle = TextStyle {
        face: FontFace::UiSmall,
        size: 12.0,
        weight: FontWeight::Regular,
    };
    /// Say text, critical-level value, compact trouble: Small 12, 600 (R§6.5).
    pub const ROW_SEMIBOLD: TextStyle = TextStyle {
        face: FontFace::UiSmall,
        size: 12.0,
        weight: FontWeight::Semibold,
    };
    /// Label: Small 11, 400 (R§6.5).
    pub const LABEL: TextStyle = TextStyle {
        face: FontFace::UiSmall,
        size: 11.0,
        weight: FontWeight::Regular,
    };
}

/// A run of text with its style and colour role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    /// The text, already formatted.
    pub text: String,
    /// Style.
    pub style: TextStyle,
    /// Colour role.
    pub role: ColorRole,
}

/// Which glyph a [`GlyphRun`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GlyphKind {
    /// A single-colour icon in [`GlyphRun::role`] (the reset glyph).
    Icon(Icon),
    /// A stacked severity glyph; its layers carry their own roles (R§6.7).
    Severity(SeverityGlyph),
}

/// A glyph in a cell: em size, cell width and the gap after it, logical px (R§6.5: reset glyph
/// 11, cell 11 + 1 gap; severity glyph 12, cell 12 + 2 gap).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlyphRun {
    /// The glyph.
    pub glyph: GlyphKind,
    /// Em size.
    pub size: f32,
    /// Cell width.
    pub cell: f32,
    /// Gap after the cell.
    pub gap_after: f32,
    /// Colour role for [`GlyphKind::Icon`]; ignored for severity glyphs.
    pub role: ColorRole,
}

/// A piece of a cell, laid out left to right.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Run {
    /// Text.
    Text(TextRun),
    /// A glyph.
    Glyph(GlyphRun),
}

/// The right cell of a two-row/single-row readout row (R§5.7). Either `say` is used (from
/// x = 0) or `value` (right-aligned in the 24 px VALUE box) and `time` (from x = 30).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RightCell {
    /// `42%`, `–`: right-aligned in the VALUE box.
    pub value: Option<TextRun>,
    /// `↺` + `14:56`, `–`, `≈` + `9d`, REASON, AGE: from x = 30. A balance's `≈` and its
    /// duration are two text runs; the renderer leaves
    /// [`crate::consts::layout::BALANCE_APPROX_GAP`] between them (R§6.3 `width("≈") + 1`).
    pub time: Vec<Run>,
    /// ▲/● + `Out 15:20`, `Reading…`, stale `Back 16:30`: from x = 0.
    pub say: Vec<Run>,
}

impl RightCell {
    /// All texts of the cell in drawing order (tests and logs).
    pub fn texts(&self) -> Vec<&str> {
        let runs = self.say.iter().chain(&self.time).filter_map(|r| match r {
            Run::Text(t) => Some(t.text.as_str()),
            Run::Glyph(_) => None,
        });
        self.value
            .iter()
            .map(|v| v.text.as_str())
            .chain(runs)
            .collect()
    }
}

/// The compact cell (R§5.8), left-aligned runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactCell {
    /// Runs from x = 0.
    pub runs: Vec<Run>,
}

/// One readout row (R§6.2): `[glyph 12] [4] [label LW] [6] [bar 40] [6] [right cell]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RowModel {
    /// The meter.
    pub key: MeterKey,
    /// Service glyph, present when rows can come from more than one service (R§6.4).
    pub glyph: Option<ProviderGlyph>,
    /// The label (`short_label`, with the account tag in M2), Small 11, TextSecondary.
    pub label: TextRun,
    /// Pace bar inputs (W = 40, rowH = 16).
    pub bar: BarInput,
    /// Right cell in the two-row and single-row forms.
    pub right: RightCell,
    /// Compact cell, used when this row is the compact form's row (R§5.8).
    pub compact: CompactCell,
    /// UI Automation name of the row (ProgressBar), R§11.
    pub uia_name: String,
    /// RangeValue `pct` (Minimum 0, Maximum 100); `None` while LOADING.
    pub uia_value: Option<u8>,
}

/// What the content allows (R§4.2), before the room on the taskbar is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentForm {
    /// Two (or more candidate) meters.
    TwoRows,
    /// Only one meter configured/available: one full row, vertically centred.
    SingleRow,
    /// Nothing to show: readout turned off, or no meter at all and no first fetch running.
    Hidden,
}

/// The form drawn, after placement (R§4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReadoutForm {
    /// Two rows.
    TwoRows,
    /// One full row.
    SingleRow,
    /// The winner among the shown rows, compact cell.
    Compact,
    /// Not shown (the tray icon remains).
    Hidden,
}

/// UI Automation data of the readout root (R§11).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadoutUia {
    /// Name in the two-row and single-row forms: `Lastdrop. ` + every row sentence.
    pub root_name: String,
    /// Name in the compact form: `Lastdrop. ` + the compact row's sentence.
    pub root_name_compact: String,
    /// HelpText: `Select for details, right-click for options.`
    pub help_text: String,
    /// Raise `UIA_LiveRegionChangedEventId` on the root with this model (a severity rose, a
    /// service turned stale or fresh, a window reset; not on value changes).
    pub announce: bool,
}

/// Everything the readout draws (R§4–R§7, R§11). Immutable; handed to the readout thread as an
/// `Arc` (ARCHITECTURE "Messages between threads"). Equal models draw equal frames, so the
/// renderer skips a redraw when the model and its own inputs (DPI, theme, hover) are unchanged
/// (R§12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadoutModel {
    /// What the content allows.
    pub content: ContentForm,
    /// Rows show the service glyph column (R§6.4).
    pub glyph_column: bool,
    /// The rows in display order (0–2).
    pub rows: Vec<RowModel>,
    /// Index into `rows` of the row the compact form shows (the winner among them, R§4.2).
    pub compact_row: usize,
    /// The strings that define the column widths (R§6.3); they change only with the locale, the
    /// pinned set or the enabled services, never with data.
    pub widths: WidthInputs,
    /// UI Automation.
    pub uia: ReadoutUia,
}
