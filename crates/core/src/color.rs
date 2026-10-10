//! Semantic colour roles (R§7, F§7). Models name a role, never a colour; `lastdrop-win`'s
//! `palette` resolves a role to ARGB for the current theme (light, dark, high contrast) and the
//! Slint `Theme` global does the same for the flyout.

use serde::{Deserialize, Serialize};

/// A colour role. Names follow the WinUI 3 theme resource they map to (R§7 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorRole {
    /// TextFillColorPrimary: text, values, calm fill on the taskbar and in the hover card, tick.
    TextPrimary,
    /// TextFillColorSecondary: labels, reset glyph, `≈`, verdicts, ages, units (R§7, F§7).
    TextSecondary,
    /// TextFillColorTertiary: sparkline past days (F§6).
    TextTertiary,
    /// TextFillColorDisabled: sparkline baseline (F§6).
    TextDisabled,
    /// The stale colour: dimmed values, dashes, status (R§6.5). TextSecondary normally,
    /// COLOR_GRAYTEXT in high contrast (R§7), which is why it is its own role.
    Stale,
    /// ControlStrongStrokeColorDefault: the pace bar track (R§6.6).
    Track,
    /// SystemFillColorCaution: warning fill and glyph (R§7).
    Caution,
    /// SystemFillColorCritical: critical fill and glyph, critical tray tile (R§7).
    Critical,
    /// SystemFillColorSuccess: success glyph in the hover card (R§6.7).
    Success,
    /// TextOnAccentFillColorPrimary: the mark inside a severity glyph, digits on a filled tile.
    TextOnAccent,
    /// AccentTextFillColorPrimary: `Sign in`, hyperlinks (R§7, F§7).
    AccentText,
    /// AccentFillColorDefault: flyout calm meter fill, sparkline today, info glyph, progress
    /// ring (F§7).
    AccentFill,
    /// SubtleFillColorSecondary: hover plate (R§6.8), button hover (F§9).
    SubtleSecondary,
    /// SubtleFillColorTertiary: pressed plate, button pressed.
    SubtleTertiary,
    /// Hover card background with transparency off: AcrylicBackgroundFillColorDefault fallback
    /// (R§7; drawn by DWM, listed for high contrast where Lastdrop paints COLOR_WINDOW).
    CardBackground,
    /// Hover card border: SurfaceStrokeColorFlyout (R§7).
    CardBorder,
    /// Divider lines: DividerStrokeColorDefault (hover card sections R§8.2, flyout footer F§5).
    Divider,
    /// Flyout service card background: CardBackgroundFillColorDefault (F§4.1).
    FlyoutCardBackground,
    /// Flyout service card border: CardStrokeColorDefault (F§4.1).
    FlyoutCardStroke,
    /// Plan badge background, a Lastdrop token (F§7 BadgeFill).
    BadgeFill,
    /// Flyout footer strip, a Lastdrop token (F§7 FooterFill).
    FooterFill,
    /// FocusStrokeColorOuter (F§9).
    FocusOuter,
    /// FocusStrokeColorInner (F§9).
    FocusInner,
    /// `0x01000000`: alpha 1/255 under the readout's content so the whole slot receives the
    /// mouse (R§6.1 "Hit testing"). Not a theme colour.
    HitTest,
}
