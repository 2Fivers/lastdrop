//! `palette::resolve` against the colour tables of R§7 and F§7 (light, dark, high contrast).
//!
//! The light/dark values are WinUI's theme resources; they were checked against the copy Windows
//! ships (`Microsoft.UI.Xaml` in `SystemApps`). One spec value disagrees with it
//! (DividerStrokeColorDefault, light) and the file wins (R§7).

use lastdrop_core::color::ColorRole as R;
use lastdrop_win::palette::{Argb, Mode, mode, resolve};
use lastdrop_win::theme::{AccentShades, HighContrastColors, ThemeInfo};

/// The owner's accent (R§7: Dark1 `#8E3AA7`, Light2 `#D58AE8`); the other shades are invented.
const ACCENT: AccentShades = AccentShades {
    accent: Argb(0xFFB1_5FCB),
    dark1: Argb(0xFF8E_3AA7),
    dark2: Argb(0xFF6B_2A80),
    light2: Argb(0xFFD5_8AE8),
    light3: Argb(0xFFE8_B9F2),
};

/// R§7's "Aquatic" contrast theme.
const AQUATIC: HighContrastColors = HighContrastColors {
    window_text: Argb(0xFFFF_FFFF),
    window: Argb(0xFF20_2020),
    highlight: Argb(0xFF8E_E3F0),
    highlight_text: Argb(0xFF26_3B50),
    gray_text: Argb(0xFFA6_A6A6),
    hotlight: Argb(0xFF75_E9FC),
};

fn theme(taskbar_dark: bool, high_contrast: Option<HighContrastColors>) -> ThemeInfo {
    ThemeInfo {
        taskbar_dark,
        // The app theme must not matter for the taskbar-themed surfaces (R§0, F§1.1).
        apps_dark: !taskbar_dark,
        accent: ACCENT,
        transparency: true,
        high_contrast,
    }
}

/// (role, light, dark) from R§7 and F§7.
const TABLE: &[(R, u32, u32)] = &[
    (R::TextPrimary, 0xE400_0000, 0xFFFF_FFFF),
    (R::TextSecondary, 0x9E00_0000, 0xC5FF_FFFF),
    (R::Stale, 0x9E00_0000, 0xC5FF_FFFF),
    (R::TextTertiary, 0x7200_0000, 0x87FF_FFFF),
    (R::TextDisabled, 0x5C00_0000, 0x5DFF_FFFF),
    (R::Track, 0x7200_0000, 0x8BFF_FFFF),
    (R::Caution, 0xFF9D_5D00, 0xFFFC_E100),
    (R::Critical, 0xFFC4_2B1C, 0xFFFF_99A4),
    (R::Success, 0xFF0F_7B0F, 0xFF6C_CB5F),
    (R::TextOnAccent, 0xFFFF_FFFF, 0xFF00_0000),
    (R::AccentText, 0xFF6B_2A80, 0xFFE8_B9F2),
    (R::AccentFill, 0xFF8E_3AA7, 0xFFD5_8AE8),
    (R::SubtleSecondary, 0x0900_0000, 0x0FFF_FFFF),
    (R::SubtleTertiary, 0x0600_0000, 0x0AFF_FFFF),
    (R::CardBackground, 0xFFF9_F9F9, 0xFF2C_2C2C),
    (R::CardBorder, 0x0F00_0000, 0x3300_0000),
    // WinUI's file says #0F000000 for light (earlier spec drafts said #14000000; R§7).
    (R::Divider, 0x0F00_0000, 0x15FF_FFFF),
    (R::FlyoutCardBackground, 0xB3FF_FFFF, 0x0DFF_FFFF),
    (R::FlyoutCardStroke, 0x0F00_0000, 0x1900_0000),
    (R::BadgeFill, 0x0F00_0000, 0x0FFF_FFFF),
    (R::FooterFill, 0x0A00_0000, 0x3000_0000),
    (R::FocusOuter, 0xE400_0000, 0xFFFF_FFFF),
    (R::FocusInner, 0xB3FF_FFFF, 0xB300_0000),
    (R::HitTest, 0x0100_0000, 0x0100_0000),
];

#[test]
fn light_and_dark_follow_the_tables() {
    let light = theme(false, None);
    let dark = theme(true, None);
    assert_eq!(mode(&light), Mode::Light);
    assert_eq!(mode(&dark), Mode::Dark);
    for &(role, l, d) in TABLE {
        assert_eq!(resolve(role, &light), Argb(l), "{role:?} light");
        assert_eq!(resolve(role, &dark), Argb(d), "{role:?} dark");
    }
}

#[test]
fn high_contrast_uses_system_colours_whatever_the_taskbar_mode() {
    for taskbar_dark in [false, true] {
        let t = theme(taskbar_dark, Some(AQUATIC));
        assert_eq!(mode(&t), Mode::HighContrast);
        let expect = |role: R| match role {
            R::TextPrimary | R::TextSecondary | R::TextTertiary | R::Track => AQUATIC.window_text,
            R::CardBorder | R::Divider | R::FlyoutCardStroke => AQUATIC.window_text,
            R::Stale | R::TextDisabled => AQUATIC.gray_text,
            R::Caution | R::Critical | R::Success | R::AccentFill => AQUATIC.highlight,
            R::SubtleSecondary | R::SubtleTertiary | R::FocusOuter => AQUATIC.highlight,
            R::TextOnAccent => AQUATIC.highlight_text,
            R::AccentText => AQUATIC.hotlight,
            R::CardBackground | R::FocusInner => AQUATIC.window,
            R::FlyoutCardBackground | R::BadgeFill | R::FooterFill => Argb::TRANSPARENT,
            R::HitTest => Argb(0x0100_0000),
        };
        for &(role, _, _) in TABLE {
            assert_eq!(resolve(role, &t), expect(role), "{role:?}");
        }
    }
}

#[test]
fn warning_and_critical_share_one_colour_in_contrast_themes() {
    // R§7: "In contrast themes warning and critical share one colour".
    let t = theme(true, Some(AQUATIC));
    assert_eq!(resolve(R::Caution, &t), resolve(R::Critical, &t));
}
