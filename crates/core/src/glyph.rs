//! Glyph ids and glyph data (R§6.4, R§6.7, R Appendix A, R§9.2).
//!
//! Icons are Segoe Fluent Icons code points (fallback Segoe MDL2 Assets, R§6.5). Severity glyphs
//! are two stacked icons, as WinUI's InfoBar builds them. Provider glyphs are Lastdrop's own
//! drawings on a 12 × 12 grid as SVG path data, so the readout renderer (Direct2D) and the Slint
//! `Path` element draw the same shapes.

use serde::{Deserialize, Serialize};

use crate::color::ColorRole;
use crate::model::ProviderId;

/// A single-colour icon from Segoe Fluent Icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Icon {
    /// U+E777 "UpdateRestore": resets at (R§5.1, R§6.7). Not U+E72C, which reads as "refreshed".
    Reset,
    /// U+E72C "Refresh": the flyout's refresh button (F§3).
    Refresh,
    /// U+E713 "Settings": the flyout footer button (F§5).
    Settings,
}

impl Icon {
    /// The code point in Segoe Fluent Icons.
    pub const fn code_point(self) -> char {
        match self {
            Icon::Reset => '\u{E777}',
            Icon::Refresh => '\u{E72C}',
            Icon::Settings => '\u{E713}',
        }
    }
}

/// A stacked severity glyph (R§6.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SeverityGlyph {
    /// ▲ warning: U+F139 triangle in Caution + U+F13B (!) in TextOnAccent.
    Warning,
    /// ● critical: U+F136 circle in Critical + U+F13C (!) in TextOnAccent.
    Critical,
    /// Success (hover card only): U+F136 in Success + U+F13E (check).
    Success,
    /// Informational (sign-in, card): U+F136 in AccentFill + U+F13F (i).
    Info,
    /// Stale (card, flyout): U+F140 (circle-slash) in the stale colour, a single layer.
    Stale,
}

/// One layer of a stacked glyph: a code point and its colour role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphLayer {
    /// Segoe Fluent Icons code point.
    pub ch: char,
    /// Colour role of this layer.
    pub role: ColorRole,
}

impl SeverityGlyph {
    /// The layers to draw, bottom first (R§6.7).
    pub const fn layers(self) -> &'static [GlyphLayer] {
        const fn l(ch: char, role: ColorRole) -> GlyphLayer {
            GlyphLayer { ch, role }
        }
        const WARNING: [GlyphLayer; 2] = [
            l('\u{F139}', ColorRole::Caution),
            l('\u{F13B}', ColorRole::TextOnAccent),
        ];
        const CRITICAL: [GlyphLayer; 2] = [
            l('\u{F136}', ColorRole::Critical),
            l('\u{F13C}', ColorRole::TextOnAccent),
        ];
        const SUCCESS: [GlyphLayer; 2] = [
            l('\u{F136}', ColorRole::Success),
            l('\u{F13E}', ColorRole::TextOnAccent),
        ];
        const INFO: [GlyphLayer; 2] = [
            l('\u{F136}', ColorRole::AccentFill),
            l('\u{F13F}', ColorRole::TextOnAccent),
        ];
        const STALE: [GlyphLayer; 1] = [l('\u{F140}', ColorRole::Stale)];
        match self {
            SeverityGlyph::Warning => &WARNING,
            SeverityGlyph::Critical => &CRITICAL,
            SeverityGlyph::Success => &SUCCESS,
            SeverityGlyph::Info => &INFO,
            SeverityGlyph::Stale => &STALE,
        }
    }
}

/// The glyph of a service (R§6.4, R§6.7).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderGlyph {
    /// A drawn glyph from R Appendix A.
    Drawn(ProviderId),
    /// A monogram for a service without a drawing: 1 letter (8 px semibold), or 2 letters at
    /// 7 px when two enabled services share the first letter, in a 1 px rounded square r = 2.5
    /// (R§6.4).
    Monogram(String),
}

/// One stroked or filled path of a provider glyph, in the 12 × 12 grid (R Appendix A).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphPath {
    /// SVG path data (M, L, H, V, C, A, Z; absolute and relative). Rects and circles of the
    /// appendix are converted to paths here so renderers need one path parser only.
    pub d: &'static str,
    /// Stroke width in grid units; `0.0` = not stroked. Scaled with the glyph (16/12 at 16 px).
    pub stroke_width: f32,
    /// Filled with the glyph colour (TextPrimary) instead of stroked.
    pub fill: bool,
    /// Round line caps (`stroke-linecap="round"`).
    pub round_caps: bool,
    /// Round line joins (`stroke-linejoin="round"`).
    pub round_joins: bool,
}

const fn stroke(d: &'static str, w: f32, caps: bool, joins: bool) -> GlyphPath {
    GlyphPath {
        d,
        stroke_width: w,
        fill: false,
        round_caps: caps,
        round_joins: joins,
    }
}

const CLAUDE_PATHS: [GlyphPath; 1] = [stroke(
    "M6 1.15v9.7M1.8 3.58l8.4 4.84M1.8 8.42l8.4-4.84",
    1.35,
    true,
    false,
)];

const CODEX_PATHS: [GlyphPath; 2] = [
    // <rect x="0.9" y="1.4" width="10.2" height="9.2" rx="2.3"/>
    stroke(
        "M3.2 1.4H8.8A2.3 2.3 0 0 1 11.1 3.7V8.3A2.3 2.3 0 0 1 8.8 10.6H3.2\
         A2.3 2.3 0 0 1 0.9 8.3V3.7A2.3 2.3 0 0 1 3.2 1.4Z",
        1.05,
        false,
        false,
    ),
    stroke("M3.35 4.35 5.15 6 3.35 7.65M6.35 8.05h2.4", 1.1, true, true),
];

const MESHY_PATHS: [GlyphPath; 2] = [
    stroke("M6 1.3 11.1 10.4H0.9z", 1.05, false, true),
    stroke("M3.45 5.85h5.1L6 10.4z", 1.0, false, true),
];

/// The paths of a drawn provider glyph (R Appendix A), drawn in TextPrimary.
pub const fn provider_paths(provider: ProviderId) -> &'static [GlyphPath] {
    match provider {
        ProviderId::Claude => &CLAUDE_PATHS,
        ProviderId::Codex => &CODEX_PATHS,
        ProviderId::Meshy => &MESHY_PATHS,
    }
}

/// The Lastdrop drop on a 16 × 16 grid (R§9.2), scaled by `size / 16`. Filled × 0.95 for
/// LOADING; stroked × 0.78 inside the EXHAUSTED tile.
pub const DROP_PATH: &str =
    "M8 1.2C8 1.2 3.2 6.9 3.2 10.1A4.8 4.8 0 0 0 12.8 10.1C12.8 6.9 8 1.2 8 1.2Z";
