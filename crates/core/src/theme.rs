//! Themes (docs/design/themes-spec.md): colours and widget layout as **data**, a JSON file per
//! theme, never Slint code (ARCHITECTURE "Slint licence rule"). Pure: parsing, the built-in
//! themes, and what a theme makes of a colour role or the flyout's widgets. The app reads and
//! writes the files; `lastdrop-win` and the Slint windows draw with the result.
//!
//! High contrast always wins: a contrast theme's system colours replace every theme colour
//! (R§7, F§7).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::color::ColorRole;

/// The id of the default theme: Windows' own look (the M1 look).
pub const DEFAULT_ID: &str = "windows";

/// The ids of the built-in themes, in the order the pickers list them.
pub const BUILT_IN: [&str; 4] = [DEFAULT_ID, "segmented", "compact", "ember"];

/// The longest theme name kept.
pub const MAX_NAME: usize = 40;

/// Which light/dark mode the taskbar-themed surfaces use (readout, hover card, tray icon,
/// flyout; R§0, F§1.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// The taskbar's mode (`SystemUsesLightTheme`).
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// How much room the flyout's cards take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// F§2's sizes.
    #[default]
    Regular,
    /// Tighter: card padding 8 instead of 12, 4 between meters instead of 8.
    Compact,
}

/// How the taskbar readout draws its bars (ROADMAP "Precise, not segmented … A segmented look
/// stays available as a theme").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarStyle {
    /// One continuous bar (R§6.6).
    #[default]
    Continuous,
    /// Ten blocks with a hairline between them.
    Segmented,
}

/// A part of a flyout card a theme can show or hide (themes-spec §2). The header widgets
/// ([`WidgetId::Plan`], [`WidgetId::Age`], [`WidgetId::Sparkline`]) are drawn in the theme's order
/// after the service's name; the others stay where F§4 puts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetId {
    /// The plan badge (`Max`, `Pro`).
    Plan,
    /// The reading's age (`1 min ago`).
    Age,
    /// The 14-day sparkline (F§6).
    Sparkline,
    /// The right cluster of a meter row: reset time or countdown (F§4.4).
    Resets,
    /// The meter bars (F§4.4).
    Bars,
    /// The trouble lines under meters in trouble (F§4.4).
    Trouble,
}

impl WidgetId {
    /// Every widget, in the default order.
    pub const ALL: [WidgetId; 6] = [
        WidgetId::Plan,
        WidgetId::Age,
        WidgetId::Sparkline,
        WidgetId::Resets,
        WidgetId::Bars,
        WidgetId::Trouble,
    ];

    /// It sits in the card header, where the theme's order applies.
    pub fn in_header(self) -> bool {
        matches!(self, WidgetId::Plan | WidgetId::Age | WidgetId::Sparkline)
    }
}

/// One widget of a theme's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Widget {
    /// Which.
    pub id: WidgetId,
    /// Shown.
    pub shown: bool,
}

/// A theme's layout (themes-spec §2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeLayout {
    /// The flyout's density.
    pub density: Density,
    /// The readout's bars.
    pub readout_bar: BarStyle,
    /// The widgets in order, each once ([`ThemeLayout::normalized`]).
    pub widgets: Vec<Widget>,
}

impl Default for ThemeLayout {
    fn default() -> ThemeLayout {
        ThemeLayout {
            density: Density::Regular,
            readout_bar: BarStyle::Continuous,
            widgets: WidgetId::ALL
                .iter()
                .map(|&id| Widget { id, shown: true })
                .collect(),
        }
    }
}

impl ThemeLayout {
    /// Every widget exactly once: the first entry of each id wins, missing ones are added shown
    /// at the end, in the default order.
    pub fn normalized(mut self) -> ThemeLayout {
        let mut seen = Vec::new();
        self.widgets.retain(|w| {
            let new = !seen.contains(&w.id);
            seen.push(w.id);
            new
        });
        for id in WidgetId::ALL {
            if !seen.contains(&id) {
                self.widgets.push(Widget { id, shown: true });
            }
        }
        self
    }

    /// Whether `id` is shown.
    pub fn shown(&self, id: WidgetId) -> bool {
        self.widgets
            .iter()
            .find(|w| w.id == id)
            .is_none_or(|w| w.shown)
    }

    /// The header widgets in the theme's order (shown or not).
    pub fn header_order(&self) -> Vec<WidgetId> {
        self.widgets
            .iter()
            .map(|w| w.id)
            .filter(|id| id.in_header())
            .collect()
    }
}

/// A colour of a theme file: `#RRGGBB` or `#RRGGBBAA`, kept as straight ARGB `0xAARRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeColor(pub u32);

impl ThemeColor {
    /// Parses `#RRGGBB` or `#RRGGBBAA` (case-insensitive); anything else is `None`.
    pub fn parse(s: &str) -> Option<ThemeColor> {
        let hex = s.trim().strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        match hex.len() {
            6 => Some(ThemeColor(0xFF00_0000 | v)),
            8 => Some(ThemeColor((v & 0xFF) << 24 | v >> 8)),
            _ => None,
        }
    }

    /// `#RRGGBB` when opaque, else `#RRGGBBAA`.
    pub fn to_hex(self) -> String {
        let (a, rgb) = (self.0 >> 24, self.0 & 0x00FF_FFFF);
        if a == 0xFF {
            format!("#{rgb:06X}")
        } else {
            format!("#{rgb:06X}{a:02X}")
        }
    }

    /// Opaque `rgb` (`0xRRGGBB`).
    pub const fn rgb(rgb: u32) -> ThemeColor {
        ThemeColor(0xFF00_0000 | (rgb & 0x00FF_FFFF))
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ThemeColor, D::Error> {
        let s = String::deserialize(d)?;
        ThemeColor::parse(&s).ok_or_else(|| serde::de::Error::custom("not a #RRGGBB colour"))
    }
}

/// The colour roles a theme may change (themes-spec §1). The others (focus strokes, hover
/// plates, the hit-test fill) follow Windows so the windows keep working as controls.
pub const THEMABLE: [ColorRole; 11] = [
    ColorRole::AccentFill,
    ColorRole::TextPrimary,
    ColorRole::TextSecondary,
    ColorRole::Track,
    ColorRole::Caution,
    ColorRole::Critical,
    ColorRole::Success,
    ColorRole::FlyoutCardBackground,
    ColorRole::FlyoutCardStroke,
    ColorRole::Divider,
    ColorRole::BadgeFill,
];

/// A theme (themes-spec §1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    /// Its name as the pickers show it.
    pub name: String,
    /// Light, dark or the taskbar's mode.
    pub mode: ThemeMode,
    /// Its own accent (`#RRGGBB`) instead of Windows' accent; the shades are derived from it.
    pub accent: Option<ThemeColor>,
    /// Colours by role for light mode ([`THEMABLE`] roles only; unknown roles and colours that
    /// do not parse are skipped).
    #[serde(deserialize_with = "lenient_colors")]
    pub light: BTreeMap<ColorRole, ThemeColor>,
    /// Colours by role for dark mode.
    #[serde(deserialize_with = "lenient_colors")]
    pub dark: BTreeMap<ColorRole, ThemeColor>,
    /// The layout.
    pub layout: ThemeLayout,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            name: "Windows".to_owned(),
            mode: ThemeMode::System,
            accent: None,
            light: BTreeMap::new(),
            dark: BTreeMap::new(),
            layout: ThemeLayout::default(),
        }
    }
}

/// A role-to-colour map that skips what it does not understand (a newer build's role, a typo),
/// so one bad entry does not throw away the theme.
fn lenient_colors<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<ColorRole, ThemeColor>, D::Error> {
    let raw = BTreeMap::<String, serde_json::Value>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(role, color)| {
            let role: ColorRole = serde_json::from_value(serde_json::Value::String(role)).ok()?;
            let color = ThemeColor::parse(color.as_str()?)?;
            Some((role, color))
        })
        .collect())
}

/// Why a theme file was not read.
#[derive(Debug)]
pub struct ThemeError(pub String);

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Theme {
    /// Parses a theme file: unknown fields are ignored, colours of roles a theme may not change
    /// are dropped, the layout is [normalized](ThemeLayout::normalized) and the name trimmed to
    /// [`MAX_NAME`] characters (an empty one becomes `fallback_name`).
    pub fn parse(json: &str, fallback_name: &str) -> Result<Theme, ThemeError> {
        let raw: serde_json::Value =
            serde_json::from_str(json).map_err(|e| ThemeError(e.to_string()))?;
        if !raw.is_object() {
            return Err(ThemeError("a theme is a JSON object".to_owned()));
        }
        let name = raw
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut theme: Theme =
            serde_json::from_value(raw).map_err(|e| ThemeError(e.to_string()))?;
        theme.light.retain(|role, _| THEMABLE.contains(role));
        theme.dark.retain(|role, _| THEMABLE.contains(role));
        theme.layout = theme.layout.normalized();
        theme.name = clean_name(&name, fallback_name);
        Ok(theme)
    }

    /// The theme as a file (pretty JSON).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The built-in theme `id` ([`BUILT_IN`]); `None` for any other id.
    pub fn built_in(id: &str) -> Option<Theme> {
        let mut theme = Theme::default();
        match id {
            DEFAULT_ID => {}
            "segmented" => {
                theme.name = "Segmented".to_owned();
                theme.layout.readout_bar = BarStyle::Segmented;
            }
            "compact" => {
                theme.name = "Compact".to_owned();
                theme.layout.density = Density::Compact;
                for w in &mut theme.layout.widgets {
                    if matches!(w.id, WidgetId::Plan | WidgetId::Sparkline) {
                        w.shown = false;
                    }
                }
            }
            "ember" => {
                theme.name = "Ember".to_owned();
                theme.accent = Some(ThemeColor::rgb(0xD9_5A_1E));
                theme
                    .light
                    .insert(ColorRole::Caution, ThemeColor::rgb(0xB3_6B_00));
                theme
                    .dark
                    .insert(ColorRole::Caution, ThemeColor::rgb(0xFF_C2_4D));
            }
            _ => return None,
        }
        Some(theme)
    }

    /// The colour this theme gives `role` in dark or light mode, if it changes it.
    pub fn color(&self, role: ColorRole, dark: bool) -> Option<ThemeColor> {
        let colors = if dark { &self.dark } else { &self.light };
        colors.get(&role).copied()
    }

    /// Whether the surfaces are dark, given the taskbar's mode.
    pub fn dark(&self, taskbar_dark: bool) -> bool {
        match self.mode {
            ThemeMode::System => taskbar_dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }
}

/// `name` trimmed, without control characters, at most [`MAX_NAME`] characters; `fallback` when
/// nothing is left.
pub fn clean_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// A file-name-safe id for a theme named `name`: lower-case ASCII letters and digits, runs of
/// anything else as one `-`, at most 40 characters; `theme` when nothing is left. Never one of
/// [`BUILT_IN`] (a `-2` is added).
pub fn id_for(name: &str) -> String {
    let mut id = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
        if id.len() >= MAX_NAME {
            break;
        }
    }
    let mut id = id.trim_matches('-').to_owned();
    if id.is_empty() {
        id = "theme".to_owned();
    }
    if BUILT_IN.contains(&id.as_str()) {
        id.push_str("-2");
    }
    id
}

/// The accent shades of a theme accent, as Windows derives its own (R§7: AccentDark1,
/// AccentDark2, AccentLight2, AccentLight3): `(dark1, dark2, light2, light3)`, straight ARGB.
/// Mixed towards black or white in sRGB: dark1 25 %, dark2 45 % black; light2 45 %, light3 65 %
/// white (close to the shades Windows derives for its preset accents).
pub fn accent_shades(accent: ThemeColor) -> (u32, u32, u32, u32) {
    let mix = |towards: u32, share: f64| {
        let channel = |shift: u32| {
            let c = f64::from((accent.0 >> shift) & 0xFF);
            let t = f64::from(towards);
            // Rounded and clamped to 0–255 first, so the cast is exact.
            ((c + (t - c) * share).round().clamp(0.0, 255.0) as u32) << shift
        };
        0xFF00_0000 | channel(16) | channel(8) | channel(0)
    };
    (mix(0, 0.25), mix(0, 0.45), mix(255, 0.45), mix(255, 0.65))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_and_print_both_ways() {
        assert_eq!(ThemeColor::parse("#D95A1E"), Some(ThemeColor(0xFFD9_5A1E)));
        assert_eq!(
            ThemeColor::parse(" #d95a1e80 "),
            Some(ThemeColor(0x80D9_5A1E))
        );
        assert_eq!(ThemeColor::parse("D95A1E"), None);
        assert_eq!(ThemeColor::parse("#D95A1"), None);
        assert_eq!(ThemeColor::parse("#GG5A1E"), None);
        assert_eq!(ThemeColor::parse("#+95A1E"), None);
        assert_eq!(ThemeColor(0xFFD9_5A1E).to_hex(), "#D95A1E");
        assert_eq!(ThemeColor(0x80D9_5A1E).to_hex(), "#D95A1E80");
    }

    #[test]
    fn a_file_round_trips_and_is_cleaned() {
        let json = r##"{
            "name": "  Mine\u0007  ",
            "mode": "dark",
            "accent": "#336699",
            "light": {"caution": "#AA5500", "hit_test": "#000000", "focus_outer": "#FFFFFF", "glow": "#FFFFFF", "track": "pink"},
            "dark": {"critical": "#FF8080CC"},
            "layout": {
                "density": "compact",
                "readout_bar": "segmented",
                "widgets": [
                    {"id": "sparkline", "shown": false},
                    {"id": "plan", "shown": true},
                    {"id": "sparkline", "shown": true}
                ]
            },
            "from_a_newer_build": 1
        }"##;
        let theme = Theme::parse(json, "Untitled").expect("theme");
        assert_eq!(theme.name, "Mine");
        assert_eq!(theme.mode, ThemeMode::Dark);
        assert_eq!(theme.light.len(), 1, "only themable roles stay");
        assert_eq!(
            theme.color(ColorRole::Critical, true),
            Some(ThemeColor(0xCCFF_8080))
        );
        assert_eq!(theme.layout.widgets.len(), WidgetId::ALL.len());
        assert!(
            !theme.layout.shown(WidgetId::Sparkline),
            "the first entry wins"
        );
        assert_eq!(
            theme.layout.header_order(),
            [WidgetId::Sparkline, WidgetId::Plan, WidgetId::Age]
        );
        let again = Theme::parse(&theme.to_json(), "x").expect("round trip");
        assert_eq!(again, theme);
        assert!(Theme::parse("[]", "x").is_err());
        assert!(Theme::parse("{\"accent\": \"red\"}", "x").is_err());
        assert_eq!(
            Theme::parse("{}", "Fallback").expect("empty").name,
            "Fallback"
        );
    }

    #[test]
    fn built_ins_exist_and_differ() {
        let all: Vec<Theme> = BUILT_IN
            .iter()
            .map(|id| Theme::built_in(id).expect("built in"))
            .collect();
        assert_eq!(all[0], Theme::default());
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(all[1].layout.readout_bar, BarStyle::Segmented);
        assert!(!all[2].layout.shown(WidgetId::Sparkline));
        assert!(Theme::built_in("nope").is_none());
    }

    #[test]
    fn ids_are_safe_and_never_built_in() {
        assert_eq!(id_for("My Theme!"), "my-theme");
        assert_eq!(id_for("  Ünïcode  "), "n-code");
        assert_eq!(id_for("..\\..\\evil"), "evil");
        assert_eq!(id_for("***"), "theme");
        assert_eq!(id_for("Windows"), "windows-2");
        assert!(id_for(&"x".repeat(100)).len() <= MAX_NAME);
    }

    #[test]
    fn modes_and_accent_shades() {
        let mut theme = Theme::default();
        assert!(theme.dark(true));
        theme.mode = ThemeMode::Light;
        assert!(!theme.dark(true));
        theme.mode = ThemeMode::Dark;
        assert!(theme.dark(false));
        let (d1, d2, l2, l3) = accent_shades(ThemeColor::rgb(0x00_80_FF));
        assert_eq!(d1, 0xFF00_60BF);
        assert_eq!(d2, 0xFF00_468C);
        assert_eq!(l2, 0xFF73_B9FF);
        assert_eq!(l3, 0xFFA6_D3FF);
    }
}
