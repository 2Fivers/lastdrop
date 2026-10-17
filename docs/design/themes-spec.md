# Themes and Theme Studio

ROADMAP M4 "Themes": built-in themes with native Windows 11 as the default, and **Theme Studio**,
a visual editor for colours and layout saved as a theme file that can be shared. Settled scope
(2026-10-16): colours and widget layout, no free-form placement.

References: readout-spec (R§), flyout-spec (F§), ARCHITECTURE "Slint licence rule" (themes are
data, never Slint code).

## 1. What a theme is

A JSON file (`lastdrop_core::theme::Theme`), unknown fields ignored:

```json
{
  "name": "Ember",
  "mode": "system",
  "accent": "#D95A1E",
  "light": { "caution": "#B36B00" },
  "dark": { "caution": "#FFC24D" },
  "layout": {
    "density": "regular",
    "readout_bar": "continuous",
    "widgets": [
      { "id": "plan", "shown": true }, { "id": "age", "shown": true },
      { "id": "sparkline", "shown": true }, { "id": "resets", "shown": true },
      { "id": "bars", "shown": true }, { "id": "trouble", "shown": true }
    ]
  }
}
```

- **mode:** `system` (the taskbar's light or dark, R§0), `light` or `dark`, for the
  taskbar-themed surfaces: readout, hover card, tray icon and flyout. The settings, about,
  history and Studio windows keep following the app mode.
- **accent:** `#RRGGBB` instead of Windows' accent; its shades are mixed towards black (25 %,
  45 %) and white (45 %, 65 %) as AccentDark1/2 and AccentLight2/3 (R§7).
- **light / dark:** colours by role, `#RRGGBB` or `#RRGGBBAA`, for these roles only
  (`lastdrop_core::theme::THEMABLE`): `accent_fill`, `text_primary`, `text_secondary`, `track`,
  `caution`, `critical`, `success`, `flyout_card_background`, `flyout_card_stroke`, `divider`,
  `badge_fill`. Focus strokes, hover plates and the hit-test fill always follow Windows, so the
  surfaces keep working as controls. The stale colour follows `text_secondary`. A role that is
  not listed, or a colour that does not parse, is skipped; the rest of the file still counts.
- **High contrast always wins:** with a contrast theme on, every colour is the system's (R§7,
  F§7) whatever the theme says; only the layout applies.
- **name:** at most 40 characters, control characters dropped; empty takes the file's id.

## 2. Layout

- **density:** `regular` (F§2) or `compact`: card padding 8 instead of 12, the body 6 below the
  header instead of 10, meters 4 apart instead of 8.
- **readout_bar:** `continuous` (R§6.6) or `segmented`: the readout's bars cut into ten blocks by
  a gap one hair wide at each tenth (`lastdrop_core::readout::bar::segmented`); the tick and its
  gaps stay, ends are square. Bars shorter than ten blocks of two pixels stay continuous. The
  flyout and hover card keep their continuous bars.
- **widgets:** the parts of a flyout card that can be shown or hidden, each once (the first entry
  of an id wins; missing ones are added, shown): `plan` (badge), `age`, `sparkline` (F§6),
  `resets` (a meter row's right cluster), `bars`, `trouble` (the trouble lines). The order of the
  three header widgets is theirs: a `plan` first stays next to the service's name, the others go
  right of the gap, 8 apart. The other widgets keep their F§4 places. A hidden widget's
  information is still in the card's accessible text and the hover card.

## 3. Where themes live

- Built in (`lastdrop_core::theme::BUILT_IN`): **Windows** (`windows`, the default and the M1
  look), **Segmented** (segmented readout bars), **Compact** (compact, no plan badge, no
  sparkline) and **Ember** (an orange accent and warmer warning colours). Their names are
  strings in the chosen language.
- The user's: `%APPDATA%\Lastdrop\themes\<id>.json`, at most 64 KB, where the id is the name in
  lower-case ASCII letters and digits joined by `-` (`theme::id_for`; never a built-in id).
  Sharing a theme is sharing its file: copy it into that folder.
- The settings' `theme` is the id (`None` = `windows`). A missing or broken file falls back to
  the Windows theme and is logged.
- The tray process reads the theme at start-up and after every settings change (the dialog
  process wrote `settings.json`), and re-applies it when it changed: `lastdrop_win::theme::apply`
  turns Windows' theme plus the Lastdrop theme into one `ThemeInfo` (`custom`: colours for the
  mode, bar style, layout), which the readout thread, the tray icon and the Slint `Theme` globals
  draw with.

## 4. Theme Studio

Settings ▸ Theme: a list of the themes (built-in first, then the user's by name) and
**Theme Studio…**, which opens the Studio window in the dialog process, starting from the theme
selected there.

- **Start from** (a theme) and **Name**.
- **Colours:** light or dark (same as the taskbar, always light, always dark); own accent colour
  (a field); one row per role with a light and a dark field, each with a swatch of the colour in
  effect (the theme's, else Windows'); a field that does not parse shows `!` and is left out.
- **Layout:** density, taskbar bars, and the card parts with a check box each and Up/Down for the
  three header widgets.
- **Preview:** the demo's first two flyout cards drawn with the draft (its mode, colours and
  layout), on the flyout's solid backdrop.
- **Save** writes the draft as `<id>.json` (replacing a file of that id); **Save and use** also
  makes it the theme of `settings.json` and tells the tray process, which applies it at once.
  **Open themes folder** opens it in File Explorer (`lastdrop_win::shell::open_folder`: an
  existing folder only). The status line says the file's name, or why it was not saved.
- `--preview-theme-studio <png>` (with `--theme <id>`, `--language <tag>`) renders the window;
  `--theme` also themes `--preview-flyout` and `--preview-hovercard`.
