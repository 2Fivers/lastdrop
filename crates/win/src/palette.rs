//! Colour roles to colours (R§7, F§7). The WinUI 3 theme resources
//! (`Common_themeresources_any.xaml`) win if a value here disagrees with them.
//!
//! Draw the ARGB tokens with their alpha on the transparent layer and let DWM blend; never
//! hard-code blended values (R§7).
//!
//! Threading: pure functions, any thread (the readout thread and the main thread each resolve
//! their own colours from their own `ThemeInfo` copy).

use lastdrop_core::color::ColorRole;

use crate::theme::ThemeInfo;

/// A straight (non-premultiplied) ARGB colour, `0xAARRGGBB`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Argb(pub u32);

impl Argb {
    /// Fully transparent.
    pub const TRANSPARENT: Argb = Argb(0);

    /// From components.
    pub const fn new(a: u8, r: u8, g: u8, b: u8) -> Argb {
        Argb((a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }

    /// Opaque from a `COLORREF` (`0x00BBGGRR`, as `GetSysColor` returns it).
    pub const fn from_colorref(c: u32) -> Argb {
        Argb::new(0xFF, c as u8, (c >> 8) as u8, (c >> 16) as u8)
    }

    /// Alpha.
    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// Red.
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    /// Green.
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    /// Blue.
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// Premultiplied BGRA bytes, the pixel format of the readout bitmap and
    /// `UpdateLayeredWindow`.
    pub const fn premultiplied_bgra(self) -> [u8; 4] {
        const fn mul(c: u8, a: u8) -> u8 {
            ((c as u32 * a as u32 + 127) / 255) as u8
        }
        let a = self.a();
        [mul(self.b(), a), mul(self.g(), a), mul(self.r(), a), a]
    }
}

/// Which column of the token tables applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Light taskbar / Windows mode.
    Light,
    /// Dark taskbar / Windows mode.
    Dark,
    /// A contrast theme (system colours).
    HighContrast,
}

/// The mode of the taskbar-themed surfaces: readout, hover card, tray icon and flyout (R§0,
/// F§1.1).
pub fn mode(theme: &ThemeInfo) -> Mode {
    match (theme.high_contrast, theme.taskbar_dark) {
        (Some(_), _) => Mode::HighContrast,
        (None, true) => Mode::Dark,
        (None, false) => Mode::Light,
    }
}

/// The colour of `role` in `theme` (R§7, F§7 tables).
///
/// High contrast (R§7, F§7): all text, calm fill, track and tick are COLOR_WINDOWTEXT; warning,
/// critical, success and accent fills COLOR_HIGHLIGHT; marks on them COLOR_HIGHLIGHTTEXT; the
/// stale colour COLOR_GRAYTEXT; `Sign in` and links COLOR_HOTLIGHT; card background
/// COLOR_WINDOW with a COLOR_WINDOWTEXT border; flyout cards, badge and footer have no fill. The
/// hover/pressed plates resolve to COLOR_HIGHLIGHT and are drawn as a 1 px **outline** instead of
/// a fill (R§6.8).
pub fn resolve(role: ColorRole, theme: &ThemeInfo) -> Argb {
    use ColorRole as R;
    if let Some(hc) = theme.high_contrast {
        return match role {
            R::TextPrimary | R::TextSecondary | R::TextTertiary | R::Track => hc.window_text,
            R::CardBorder | R::Divider | R::FlyoutCardStroke => hc.window_text,
            R::Stale | R::TextDisabled => hc.gray_text,
            R::Caution | R::Critical | R::Success | R::AccentFill => hc.highlight,
            R::SubtleSecondary | R::SubtleTertiary | R::FocusOuter => hc.highlight,
            R::TextOnAccent => hc.highlight_text,
            R::AccentText => hc.hotlight,
            R::CardBackground | R::FocusInner => hc.window,
            R::FlyoutCardBackground | R::BadgeFill | R::FooterFill => Argb::TRANSPARENT,
            R::HitTest => Argb(0x0100_0000),
        };
    }
    let dark = theme.taskbar_dark;
    let pick = |light: u32, dark_value: u32| Argb(if dark { dark_value } else { light });
    match role {
        R::TextPrimary => pick(0xE400_0000, 0xFFFF_FFFF),
        R::TextSecondary | R::Stale => pick(0x9E00_0000, 0xC5FF_FFFF),
        R::TextTertiary => pick(0x7200_0000, 0x87FF_FFFF),
        R::TextDisabled => pick(0x5C00_0000, 0x5DFF_FFFF),
        R::Track => pick(0x7200_0000, 0x8BFF_FFFF),
        R::Caution => pick(0xFF9D_5D00, 0xFFFC_E100),
        R::Critical => pick(0xFFC4_2B1C, 0xFFFF_99A4),
        R::Success => pick(0xFF0F_7B0F, 0xFF6C_CB5F),
        R::TextOnAccent => pick(0xFFFF_FFFF, 0xFF00_0000),
        R::AccentText => {
            if dark {
                theme.accent.light3
            } else {
                theme.accent.dark2
            }
        }
        R::AccentFill => {
            if dark {
                theme.accent.light2
            } else {
                theme.accent.dark1
            }
        }
        R::SubtleSecondary => pick(0x0900_0000, 0x0FFF_FFFF),
        R::SubtleTertiary => pick(0x0600_0000, 0x0AFF_FFFF),
        R::CardBackground => pick(0xFFF9_F9F9, 0xFF2C_2C2C),
        R::CardBorder => pick(0x0F00_0000, 0x3300_0000),
        R::Divider => pick(0x1400_0000, 0x15FF_FFFF),
        R::FlyoutCardBackground => pick(0xB3FF_FFFF, 0x0DFF_FFFF),
        R::FlyoutCardStroke => pick(0x0F00_0000, 0x1900_0000),
        R::BadgeFill => pick(0x0F00_0000, 0x0FFF_FFFF),
        R::FooterFill => pick(0x0A00_0000, 0x3000_0000),
        R::FocusOuter => pick(0xE400_0000, 0xFFFF_FFFF),
        R::FocusInner => pick(0xB3FF_FFFF, 0xB300_0000),
        R::HitTest => Argb(0x0100_0000),
    }
}

#[cfg(test)]
mod tests {
    use super::Argb;

    #[test]
    fn premultiplies_and_converts_colorrefs() {
        assert_eq!(Argb(0xFFFF_FFFF).premultiplied_bgra(), [255, 255, 255, 255]);
        assert_eq!(Argb(0x8000_00FF).premultiplied_bgra(), [128, 0, 0, 128]);
        assert_eq!(Argb(0x0100_0000).premultiplied_bgra(), [0, 0, 0, 1]);
        assert_eq!(Argb::from_colorref(0x0050_3B26), Argb(0xFF26_3B50));
    }
}
