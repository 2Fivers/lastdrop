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
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use windows::Win32::Foundation::{LPARAM, WPARAM};

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

/// Reads the current theme (registry `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\
/// Personalize`, `UISettings`, `SystemParametersInfoW(SPI_GETHIGHCONTRAST)`, `GetSysColor`).
pub fn read() -> ThemeInfo {
    todo!("M1: read theme (spike: read_theme)")
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
pub fn classify(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<ThemeChange> {
    todo!("M1: classify theme messages")
}
