//! Theme: taskbar and app light/dark, accent shades, transparency, high contrast; and the
//! messages that announce a change (R§0 "Taskbar theme", R§7, F§1.1).
//!
//! The readout, its hover card, the tray icon **and the flyout** follow the taskbar/Windows mode
//! (`SystemUsesLightTheme`), never `AppsUseLightTheme` (R§0, F§1.1); `apps_dark` is for the
//! settings and about windows only.
//!
//! Threading: [`read`] may run on any thread with COM initialized (it uses
//! `UISettings.GetColorValue`); [`classify`] is called from the window procedures of the readout
//! thread and the main thread.

use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW,
    COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SPI_SETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW, WM_DWMCOLORIZATIONCOLORCHANGED, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE,
    WM_THEMECHANGED,
};
use windows::core::{BOOL, w};

use crate::palette::Argb;

/// The system accent and its shades (`UISettings.GetColorValue`, R§7: "Accent shades come from
/// AccentDark1/AccentDark2/AccentLight2/AccentLight3"). Falls back to
/// `DwmGetColorizationColor`, then `#005FB8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccentShades {
    /// `UIColorType::Accent`.
    pub accent: Argb,
    /// AccentDark1: AccentFill in light mode.
    pub dark1: Argb,
    /// AccentDark2: AccentText in light mode.
    pub dark2: Argb,
    /// AccentLight2: AccentFill in dark mode.
    pub light2: Argb,
    /// AccentLight3: AccentText in dark mode.
    pub light3: Argb,
}

/// High-contrast system colours (`SPI_GETHIGHCONTRAST` on; `GetSysColor`, re-read on
/// `WM_SYSCOLORCHANGE`; R§7 high-contrast table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HighContrastColors {
    /// COLOR_WINDOWTEXT: all text, calm fill, track, tick, borders.
    pub window_text: Argb,
    /// COLOR_WINDOW: card background.
    pub window: Argb,
    /// COLOR_HIGHLIGHT: warning and critical fill, glyph outer shape, tray tile, focus.
    pub highlight: Argb,
    /// COLOR_HIGHLIGHTTEXT: glyph mark, digits on a filled tile.
    pub highlight_text: Argb,
    /// COLOR_GRAYTEXT: the stale colour.
    pub gray_text: Argb,
    /// COLOR_HOTLIGHT: `Sign in`, hyperlinks.
    pub hotlight: Argb,
}

/// Everything about the theme that changes how Lastdrop draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeInfo {
    /// `SystemUsesLightTheme == 0`: taskbar, readout, hover card, tray icon, flyout are dark.
    pub taskbar_dark: bool,
    /// `AppsUseLightTheme == 0`: settings and about windows are dark.
    pub apps_dark: bool,
    /// Accent shades.
    pub accent: AccentShades,
    /// Transparency effects on (`EnableTransparency`): DWM acrylic; with it off DWM draws the
    /// solid fallback. Changes nothing in the readout (R§7).
    pub transparency: bool,
    /// High-contrast colours when a contrast theme is on.
    pub high_contrast: Option<HighContrastColors>,
}

/// `HKCU\…\Themes\Personalize`, where Windows keeps the light/dark and transparency switches.
const PERSONALIZE: windows::core::PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");

/// The accent when neither `UISettings` nor DWM answers: Windows 11's default blue
/// (AccentFillColorDefault light, `#005FB8`).
const FALLBACK_ACCENT: Argb = Argb(0xFF00_5FB8);

/// Reads the current theme (registry `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\
/// Personalize`, `UISettings`, `SystemParametersInfoW(SPI_GETHIGHCONTRAST)`, `GetSysColor`).
///
/// Missing values take Windows 11's defaults: dark taskbar, light apps, transparency on.
pub fn read() -> ThemeInfo {
    let taskbar_dark = personalize_dword(w!("SystemUsesLightTheme")).is_none_or(|v| v == 0);
    let apps_dark = personalize_dword(w!("AppsUseLightTheme")).is_some_and(|v| v == 0);
    let transparency = personalize_dword(w!("EnableTransparency")).is_none_or(|v| v != 0);
    ThemeInfo {
        taskbar_dark,
        apps_dark,
        accent: read_accent(),
        transparency,
        high_contrast: read_high_contrast(),
    }
}

/// A DWORD under [`PERSONALIZE`]; `None` when the value is missing or not a DWORD.
fn personalize_dword(name: windows::core::PCWSTR) -> Option<u32> {
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `data` is a live u32 and `size` says exactly that many bytes; RRF_RT_REG_DWORD makes
    // the call fail rather than write anything that is not a 4-byte DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut data).cast()),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

/// The accent shades from `UISettings`; DWM's colorization colour (or `#005FB8`) for every shade
/// when `UISettings` is unavailable.
fn read_accent() -> AccentShades {
    match accent_from_ui_settings() {
        Ok(shades) => shades,
        Err(e) => {
            log::warn!("theme: UISettings accent unavailable ({e}); using the DWM colour");
            let c = dwm_colorization().unwrap_or(FALLBACK_ACCENT);
            AccentShades {
                accent: c,
                dark1: c,
                dark2: c,
                light2: c,
                light3: c,
            }
        }
    }
}

fn accent_from_ui_settings() -> windows::core::Result<AccentShades> {
    let settings = UISettings::new()?;
    let get = |kind: UIColorType| -> windows::core::Result<Argb> {
        let c = settings.GetColorValue(kind)?;
        // Accent colours are opaque; force it so a stray alpha never makes a fill translucent.
        Ok(Argb::new(0xFF, c.R, c.G, c.B))
    };
    Ok(AccentShades {
        accent: get(UIColorType::Accent)?,
        dark1: get(UIColorType::AccentDark1)?,
        dark2: get(UIColorType::AccentDark2)?,
        light2: get(UIColorType::AccentLight2)?,
        light3: get(UIColorType::AccentLight3)?,
    })
}

fn dwm_colorization() -> Option<Argb> {
    let mut argb = 0u32;
    let mut opaque = BOOL::default();
    // SAFETY: both out-pointers are live locals of the documented types.
    unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.ok()?;
    Some(Argb(argb | 0xFF00_0000))
}

/// The contrast theme's colours, or `None` when no contrast theme is on.
fn read_high_contrast() -> Option<HighContrastColors> {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: `hc` is a live HIGHCONTRASTW whose cbSize is set, and uiParam is its size, as
    // SPI_GETHIGHCONTRAST requires. Windows only writes into the struct (lpszDefaultScheme points
    // into system memory and is not read here).
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some(std::ptr::from_mut(&mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if let Err(e) = read {
        log::warn!("theme: SPI_GETHIGHCONTRAST failed ({e}); assuming no contrast theme");
        return None;
    }
    if !hc.dwFlags.contains(HCF_HIGHCONTRASTON) {
        return None;
    }
    let sys = |index: SYS_COLOR_INDEX| {
        // SAFETY: GetSysColor takes any index and returns a COLORREF; no pointers involved.
        Argb::from_colorref(unsafe { GetSysColor(index) })
    };
    Some(HighContrastColors {
        window_text: sys(COLOR_WINDOWTEXT),
        window: sys(COLOR_WINDOW),
        highlight: sys(COLOR_HIGHLIGHT),
        highlight_text: sys(COLOR_HIGHLIGHTTEXT),
        gray_text: sys(COLOR_GRAYTEXT),
        hotlight: sys(COLOR_HOTLIGHT),
    })
}

/// What changed, as far as drawing is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeChange {
    /// `WM_SETTINGCHANGE` "ImmersiveColorSet": light/dark or accent; also
    /// `WM_DWMCOLORIZATIONCOLORCHANGED`.
    ColorSet,
    /// `WM_SETTINGCHANGE` with `SPI_SETHIGHCONTRAST`, or `WM_THEMECHANGED`.
    HighContrast,
    /// `WM_SYSCOLORCHANGE`: re-read the high-contrast colours.
    SysColors,
}

/// Classifies a window message as a theme change (`None` when it is not one). Callers re-read
/// [`read`] and re-render (R§12 "theme/accent/HC change").
///
/// Pass the message exactly as the window procedure received it: for `WM_SETTINGCHANGE`,
/// `lparam` is either 0 or a pointer to a NUL-terminated UTF-16 string (the documented contract
/// of that message), and it is read.
pub fn classify(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<ThemeChange> {
    match msg {
        WM_SETTINGCHANGE if wparam.0 == SPI_SETHIGHCONTRAST.0 as usize => {
            Some(ThemeChange::HighContrast)
        }
        WM_SETTINGCHANGE if lparam.0 != 0 => {
            // SAFETY: for WM_SETTINGCHANGE a non-zero lParam points to a NUL-terminated wide
            // string owned by the sender for the duration of the message (the caller passes the
            // message it is handling). `wide_str_eq` stops at the first NUL or mismatch, so it
            // never reads past the string.
            let is_color_set = unsafe { wide_str_eq(lparam.0 as *const u16, "ImmersiveColorSet") };
            is_color_set.then_some(ThemeChange::ColorSet)
        }
        WM_DWMCOLORIZATIONCOLORCHANGED => Some(ThemeChange::ColorSet),
        WM_THEMECHANGED => Some(ThemeChange::HighContrast),
        WM_SYSCOLORCHANGE => Some(ThemeChange::SysColors),
        _ => None,
    }
}

/// Compares a NUL-terminated UTF-16 string with an ASCII `expected`, reading at most
/// `expected.len() + 1` units.
///
/// # Safety
///
/// `p` must point to a NUL-terminated UTF-16 string, or to at least `expected.len() + 1`
/// readable `u16`s.
unsafe fn wide_str_eq(p: *const u16, expected: &str) -> bool {
    for (i, unit) in expected
        .encode_utf16()
        .chain(std::iter::once(0))
        .enumerate()
    {
        // SAFETY: the caller guarantees the string is NUL-terminated; we stop at the first
        // mismatch, and a NUL in the string mismatches every unit of `expected` but the last, so
        // index `i` is never past the terminator.
        let actual = unsafe { *p.add(i) };
        if actual != unit {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[test]
    fn classifies_setting_changes_by_their_string() {
        let color = wide("ImmersiveColorSet");
        let intl = wide("intl");
        let longer = wide("ImmersiveColorSetX");
        let shorter = wide("Immersive");
        let at = |s: &Vec<u16>| LPARAM(s.as_ptr() as isize);
        assert_eq!(
            classify(WM_SETTINGCHANGE, WPARAM(0), at(&color)),
            Some(ThemeChange::ColorSet)
        );
        assert_eq!(classify(WM_SETTINGCHANGE, WPARAM(0), at(&intl)), None);
        assert_eq!(classify(WM_SETTINGCHANGE, WPARAM(0), at(&longer)), None);
        assert_eq!(classify(WM_SETTINGCHANGE, WPARAM(0), at(&shorter)), None);
        assert_eq!(classify(WM_SETTINGCHANGE, WPARAM(0), LPARAM(0)), None);
    }

    #[test]
    fn classifies_high_contrast_and_colour_messages() {
        let hc = WPARAM(SPI_SETHIGHCONTRAST.0 as usize);
        assert_eq!(
            classify(WM_SETTINGCHANGE, hc, LPARAM(0)),
            Some(ThemeChange::HighContrast)
        );
        assert_eq!(
            classify(WM_THEMECHANGED, WPARAM(0), LPARAM(0)),
            Some(ThemeChange::HighContrast)
        );
        assert_eq!(
            classify(
                WM_DWMCOLORIZATIONCOLORCHANGED,
                WPARAM(0x00FF_0000),
                LPARAM(1)
            ),
            Some(ThemeChange::ColorSet)
        );
        assert_eq!(
            classify(WM_SYSCOLORCHANGE, WPARAM(0), LPARAM(0)),
            Some(ThemeChange::SysColors)
        );
        // WM_PAINT and friends are not theme messages.
        assert_eq!(classify(0x000F, WPARAM(0), LPARAM(0)), None);
    }

    #[test]
    fn reads_a_consistent_theme() {
        let t = read();
        for c in [
            t.accent.accent,
            t.accent.dark1,
            t.accent.dark2,
            t.accent.light2,
            t.accent.light3,
        ] {
            assert_eq!(c.a(), 0xFF, "accent shades are opaque");
        }
        if let Some(hc) = t.high_contrast {
            assert_eq!(hc.window_text.a(), 0xFF);
            assert_eq!(hc.window.a(), 0xFF);
        }
    }
}
