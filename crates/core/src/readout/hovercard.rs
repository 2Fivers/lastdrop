//! The hover card: a tooltip-style card over the readout or the tray icon, the readout's legend
//! (R§8). Drawn by the Slint `HoverCard` window (crates/app/ui/hovercard.slint), whose structs
//! mirror these types.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use serde::{Deserialize, Serialize};

use super::GlyphKind;
use super::bar::BarInput;
use super::build::BuildContext;
use crate::color::ColorRole;
use crate::glyph::{ProviderGlyph, SeverityGlyph};
use crate::model::MeterKey;
use crate::stats::Evaluation;

/// What the pointer rests on (R§8.2 "Which meters").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HoverAnchor {
    /// The readout: its rows, grouped by service in readout order, then "Also tracked".
    Readout,
    /// The tray icon. With the readout hidden: the winner in full, then "Also tracked"; with the
    /// readout visible: as over the readout.
    TrayIcon {
        /// The readout is currently drawn (not HIDDEN).
        readout_visible: bool,
    },
}

/// The card header (R§8.2): severity glyph 16 px (cell 16 × 20) + 8 + title (Text 14/20
/// semibold, TextPrimary). Verdict and sentence follow the worst row (EXHAUSTED > CRITICAL >
/// WARNING > STALE > OK).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardHeader {
    /// Success (OK), Warning, Critical (critical and exhausted), Stale, Info (sign-in); `None`
    /// while everything is LOADING.
    pub glyph: Option<SeverityGlyph>,
    /// `card.title.*` (Appendix B).
    pub title: String,
}

/// A service section (R§8.2): margin-top 12, divider, padding-top 10; service row 16 high.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardService {
    /// Provider glyph, 16 px.
    pub glyph: ProviderGlyph,
    /// Name, 12/16 semibold (`Claude`, M2 `Claude · Work`).
    pub name: String,
    /// Plan, 12/16 TextSecondary; `None` when unknown.
    pub plan: Option<String>,
    /// Right-aligned age, 12/16 TextSecondary: `Updated 1 min ago`; stale: `Last reading 12:42`.
    pub age: String,
    /// Meter blocks.
    pub meters: Vec<CardMeter>,
}

/// A meter block (R§8.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CardMeter {
    /// A window meter.
    Window(CardWindowMeter),
    /// A balance meter.
    Balance(CardBalanceMeter),
}

/// A window meter block (R§8.2): row 16 (name, value); bar 296 × row 11; tick line; reset line.
/// Every block shows both keys (tick key, reset key), every time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardWindowMeter {
    /// Meter `name`, 12/16 semibold, left.
    pub name: String,
    /// `42% used`, right; semibold when the meter is in trouble.
    pub value: String,
    /// `value` is semibold.
    pub value_semibold: bool,
    /// The bar (calm fill TextPrimary: the card is the readout's key).
    pub bar: BarInput,
    /// After the tick key: `55% of time gone` (stale: `… at 12:42`); IDLE `window not started`;
    /// `L` unknown `window length unknown`.
    pub elapsed_line: String,
    /// Right of the tick line: `on course for 78% at reset`, `lasts to the reset`,
    /// `no forecast yet`, `runs out ≈ 15:20`, `used up`; `None` for STALE and IDLE.
    pub forecast: Option<String>,
    /// `forecast` is semibold (WARNING or CRITICAL).
    pub forecast_semibold: bool,
    /// After the reset key: `Resets 14:56`; IDLE `Starts with your next use`; `R` none
    /// `No reset time given`; JUST_RESET `Reset at 09:00 · waiting for the first reading`.
    pub reset_line: String,
    /// Right of the reset line: `in 2 h 14 min`; `None` when there is no reset time.
    pub reset_in: Option<String>,
}

/// A balance meter block (R§8.2): row 16; bar 296 without tick; amount line; rate line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardBalanceMeter {
    /// `Credits`, 12/16 semibold.
    pub name: String,
    /// `62% of top-up used`; semibold in trouble.
    pub value: String,
    /// `value` is semibold.
    pub value_semibold: bool,
    /// The bar, no tick.
    pub bar: BarInput,
    /// `1,240 of 3,260 credits left` (TextSecondary).
    pub left_line: String,
    /// `lasts ≈ 9 days` / `runs out ≈ Thu` / `empty` / `no use in 7 days` /
    /// `not enough history yet`.
    pub forecast: String,
    /// `forecast` is semibold (WARNING/CRITICAL).
    pub forecast_semibold: bool,
    /// `136 a day, last 7 days`; `None` without a forecast.
    pub rate_line: Option<String>,
    /// `until ≈ Mon 19 Oct`; `None` without a forecast.
    pub until: Option<String>,
}

/// One "Also tracked" line, 20 px (R§8.2): glyph 12, 6, `{service} {short_name}`, value right,
/// then the time column (as wide as its widest entry, at least 56 px).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlsoLine {
    /// Provider glyph, 12 px.
    pub glyph: ProviderGlyph,
    /// `Claude 5-hour`, `Codex weekly`, `Meshy credits`.
    pub label: String,
    /// `42%`; balance: the amount `1,240`.
    pub value: String,
    /// `value` is semibold (in trouble).
    pub value_semibold: bool,
    /// The time part of the meter's two-row readout cell.
    pub time: AlsoTime,
}

/// The time column of an "Also tracked" line (R§8.2): one optional glyph and one text, so the
/// Slint `AlsoLineData` maps 1:1 (`time-glyph`, `time-text`, `time-semibold`, `time-dim`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlsoTime {
    /// ↺ (`Icon::Reset`, TextSecondary) before a reset time; ▲/● before say-text; `None` for
    /// `≈ 9 days`, `–` and stale statuses.
    pub glyph: Option<GlyphKind>,
    /// `14:56`, `Out 15:20`, `12 min left`, `Back 16:30`, `≈ 9 days`, `–`, `47m ago`.
    pub text: String,
    /// Say-text (after ▲/●) is semibold, as in the readout (R§6.5).
    pub semibold: bool,
    /// TextPrimary; TextSecondary for `–`; Stale for a stale service's status; AccentText for
    /// `Sign in`.
    pub role: ColorRole,
}

/// The "Also tracked" section (R§8.2): every enabled meter that is not a readout row; the 4
/// highest by rank (balances at any severity) in service/meter order, then `+N more`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlsoTracked {
    /// `Also tracked`.
    pub header: String,
    /// Up to `ALSO_TRACKED_MAX` lines.
    pub lines: Vec<AlsoLine>,
    /// `+N more`, when more meters exist.
    pub more: Option<String>,
}

/// The hover card (R§8). Width 320, padding 12, height = content. When the content is taller
/// than the space, the window drops "Also tracked" lines from the bottom, then the sentence
/// (R§8.1); the model always carries everything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoverCardModel {
    /// Header.
    pub header: CardHeader,
    /// The sentence (Appendix B `card.sentence.*`), margin-top 4, 12/16, wraps.
    pub sentence: String,
    /// Service sections, readout order.
    pub services: Vec<CardService>,
    /// "Also tracked"; `None` when every enabled meter is already shown.
    pub also: Option<AlsoTracked>,
    /// UIA ToolTip name: title + sentence (R§11), raised as `ToolTipOpened`.
    pub uia_name: String,
}

/// Builds the hover card for `anchor` (R§8.2). `readout_rows` are the rows the readout currently
/// shows (from the last `PlacementChanged`: the compact form shows one).
pub fn build_hover_card(
    eval: &Evaluation,
    anchor: HoverAnchor,
    readout_rows: &[MeterKey],
    cx: &BuildContext<'_>,
) -> HoverCardModel {
    todo!("M1: R§8")
}
