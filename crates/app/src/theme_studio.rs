//! Theme Studio (docs/design/themes-spec.md §4) in the dialog process: a draft theme, edited in
//! the window, drawn on demo flyout cards with the draft's colours and layout, saved as a JSON
//! file in `%APPDATA%\Lastdrop\themes`, and, with "Save and use", made the theme of the settings
//! (one read-merge-write under the settings lock, settings-spec §6; the tray process is told, as
//! after a change in Settings).
//!
//! Threading: main thread only; the theme files are a few kilobytes.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use lastdrop_core::color::ColorRole;
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Msg, Strings};
use lastdrop_core::settings::Settings;
use lastdrop_core::theme::{BarStyle, Density, THEMABLE, Theme, ThemeColor, ThemeMode, WidgetId};
use lastdrop_win::theme::ThemeInfo;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::dialog::{self, Changes};
use crate::persist::{self, Paths};
use crate::settings_sync::SettingsFile;
use crate::settings_window::Wording;
use crate::themes::{self, ThemeEntry};
use crate::{AppError, demo, ui, wiring};

/// The label of a themable role.
fn role_label(role: ColorRole, strings: &dyn Strings) -> String {
    let msg = match role {
        ColorRole::AccentFill => Msg::StudioRoleAccentFill,
        ColorRole::TextPrimary => Msg::StudioRoleTextPrimary,
        ColorRole::TextSecondary => Msg::StudioRoleTextSecondary,
        ColorRole::Track => Msg::StudioRoleTrack,
        ColorRole::Caution => Msg::StudioRoleCaution,
        ColorRole::Critical => Msg::StudioRoleCritical,
        ColorRole::Success => Msg::StudioRoleSuccess,
        ColorRole::FlyoutCardBackground => Msg::StudioRoleCardBackground,
        ColorRole::FlyoutCardStroke => Msg::StudioRoleCardBorder,
        ColorRole::Divider => Msg::StudioRoleDivider,
        _ => Msg::StudioRoleBadge,
    };
    strings.template(msg).to_owned()
}

/// The label of a widget.
fn widget_label(id: WidgetId, strings: &dyn Strings) -> String {
    let msg = match id {
        WidgetId::Plan => Msg::StudioWidgetPlan,
        WidgetId::Age => Msg::StudioWidgetAge,
        WidgetId::Sparkline => Msg::StudioWidgetSparkline,
        WidgetId::Resets => Msg::StudioWidgetResets,
        WidgetId::Bars => Msg::StudioWidgetBars,
        WidgetId::Trouble => Msg::StudioWidgetTrouble,
    };
    strings.template(msg).to_owned()
}

/// The window's texts.
fn texts(strings: &dyn Strings) -> ui::StudioTexts {
    let t = |msg: Msg| SharedString::from(strings.template(msg));
    let list = |msgs: &[Msg]| model(msgs.iter().map(|m| t(*m)).collect());
    ui::StudioTexts {
        title: t(Msg::StudioTitle),
        start_from: t(Msg::StudioStartFrom),
        name: t(Msg::StudioName),
        colours: t(Msg::StudioColours),
        mode: t(Msg::StudioMode),
        modes: list(&[
            Msg::StudioModeSystem,
            Msg::StudioModeLight,
            Msg::StudioModeDark,
        ]),
        accent_own: t(Msg::StudioAccentOwn),
        roles_hint: t(Msg::StudioRolesHint),
        light: t(Msg::StudioLight),
        dark: t(Msg::StudioDark),
        layout: t(Msg::StudioLayout),
        density: t(Msg::StudioDensity),
        densities: list(&[Msg::StudioDensityRegular, Msg::StudioDensityCompact]),
        bar: t(Msg::StudioBar),
        bars: list(&[Msg::StudioBarContinuous, Msg::StudioBarSegmented]),
        widgets: t(Msg::StudioWidgets),
        widgets_hint: t(Msg::StudioWidgetsHint),
        up: t(Msg::StudioUp),
        down: t(Msg::StudioDown),
        preview: t(Msg::StudioPreview),
        save: t(Msg::StudioSave),
        save_use: t(Msg::StudioSaveUse),
        folder: t(Msg::StudioFolder),
        close: t(Msg::StudioClose),
    }
}

/// A `Vec` as a Slint model.
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

/// A straight ARGB colour as a Slint colour.
fn slint_color(argb: u32) -> slint::Color {
    slint::Color::from_argb_encoded(argb)
}

/// The colour fields as typed: the role's colour in the draft, else empty.
#[derive(Clone, Debug, Default, PartialEq)]
struct Typed {
    /// By role ([`THEMABLE`] order): light, dark.
    roles: Vec<(String, String)>,
    accent: String,
}

impl Typed {
    fn of(theme: &Theme) -> Typed {
        let hex = |c: Option<ThemeColor>| c.map(ThemeColor::to_hex).unwrap_or_default();
        Typed {
            roles: THEMABLE
                .iter()
                .map(|r| (hex(theme.color(*r, false)), hex(theme.color(*r, true))))
                .collect(),
            accent: hex(theme.accent),
        }
    }
}

/// A typed colour: empty = none (Windows' colour), else it must parse.
fn parse_field(text: &str) -> Result<Option<ThemeColor>, ()> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    ThemeColor::parse(text).map(Some).ok_or(())
}

/// The window while it is open (main thread).
struct Session {
    window: ui::ThemeStudioWindow,
    paths: Paths,
    wording: Wording,
    /// The themes the "Start from" list shows.
    entries: RefCell<Vec<ThemeEntry>>,
    /// The draft.
    draft: RefCell<Theme>,
    /// The colour fields as typed (they may not parse yet).
    typed: RefCell<Typed>,
    /// Windows' theme now: the preview applies the draft over it.
    windows: ThemeInfo,
}

thread_local! {
    static SESSION: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

fn with_session(f: impl FnOnce(&Session)) {
    let session = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = session {
        f(&session);
    }
}

impl Session {
    fn strings(&self) -> &'static dyn Strings {
        self.wording.strings()
    }

    /// Fills every field from the draft.
    fn fill_fields(&self) {
        let draft = self.draft.borrow();
        let w = &self.window;
        w.set_name(draft.name.as_str().into());
        w.set_mode_index(match draft.mode {
            ThemeMode::System => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        });
        w.set_own_accent(draft.accent.is_some());
        w.set_density_index(i32::from(draft.layout.density == Density::Compact));
        w.set_bar_index(i32::from(draft.layout.readout_bar == BarStyle::Segmented));
        drop(draft);
        let typed = Typed::of(&self.draft.borrow());
        w.set_accent(typed.accent.as_str().into());
        *self.typed.borrow_mut() = typed;
        self.fill_rows();
    }

    /// The colour rows, the accent swatch, the widget rows and the preview from the draft and
    /// the typed fields.
    fn fill_rows(&self) {
        let strings = self.strings();
        let typed = self.typed.borrow();
        let info = lastdrop_win::theme::apply(self.windows, &self.draft.borrow());
        let light = ThemeInfo {
            taskbar_dark: false,
            ..self.windows
        };
        let dark = ThemeInfo {
            taskbar_dark: true,
            ..self.windows
        };
        let draft = self.draft.borrow();
        let shown = |base: ThemeInfo, role: ColorRole| {
            let mut probe = draft.clone();
            probe.mode = if base.taskbar_dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            let applied = lastdrop_win::theme::apply(base, &probe);
            slint_color(lastdrop_win::palette::resolve(role, &applied).0)
        };
        let rows: Vec<ui::RoleRow> = THEMABLE
            .iter()
            .zip(typed.roles.iter())
            .map(|(role, (l, d))| ui::RoleRow {
                label: role_label(*role, strings).into(),
                light: l.as_str().into(),
                dark: d.as_str().into(),
                light_ok: parse_field(l).is_ok(),
                dark_ok: parse_field(d).is_ok(),
                light_swatch: shown(light, *role),
                dark_swatch: shown(dark, *role),
            })
            .collect();
        self.window.set_roles(model(rows));
        self.window
            .set_accent_ok(parse_field(&typed.accent).is_ok_and(|c| c.is_some()));
        self.window
            .set_accent_swatch(slint_color(info.accent.accent.0));
        let widgets = &draft.layout.widgets;
        let header: Vec<usize> = (0..widgets.len())
            .filter(|i| widgets[*i].id.in_header())
            .collect();
        let rows: Vec<ui::WidgetRow> = widgets
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let place = header.iter().position(|h| *h == i);
                ui::WidgetRow {
                    name: widget_label(w.id, strings).into(),
                    shown: w.shown,
                    movable: place.is_some(),
                    can_up: place.is_some_and(|p| p > 0),
                    can_down: place.is_some_and(|p| p + 1 < header.len()),
                }
            })
            .collect();
        self.window.set_widgets(model(rows));
        drop(draft);
        self.preview(&info);
    }

    /// Draws the demo cards with the draft.
    fn preview(&self, info: &ThemeInfo) {
        let global = self.window.global::<ui::Theme>();
        wiring::set_theme(&global, info);
        global.set_solid_backdrop(crate::preview::solid_backdrop(info.taskbar_dark));
        let world = demo::Demo::scenario(info.taskbar_dark, demo::Scenario::default())
            .in_language(self.wording.language);
        let flyout = world.flyout();
        let scale = f64::from(self.window.window().scale_factor());
        let cards: Vec<ui::CardData> = flyout
            .cards
            .iter()
            .take(2)
            .map(|c| wiring::card_data(c, scale, &wiring::CardMenu::default()))
            .collect();
        self.window.set_cards(model(cards));
    }

    /// Reads the window's fields into the draft; colours that do not parse are left out.
    fn read_fields(&self) {
        let w = &self.window;
        let mut typed = self.typed.borrow_mut();
        typed.accent = w.get_accent().to_string();
        let mut draft = self.draft.borrow_mut();
        draft.name = w.get_name().to_string();
        draft.mode = match w.get_mode_index() {
            1 => ThemeMode::Light,
            2 => ThemeMode::Dark,
            _ => ThemeMode::System,
        };
        draft.accent = if w.get_own_accent() {
            parse_field(&typed.accent).ok().flatten()
        } else {
            None
        };
        draft.layout.density = if w.get_density_index() == 1 {
            Density::Compact
        } else {
            Density::Regular
        };
        draft.layout.readout_bar = if w.get_bar_index() == 1 {
            BarStyle::Segmented
        } else {
            BarStyle::Continuous
        };
        draft.light.clear();
        draft.dark.clear();
        for (role, (l, d)) in THEMABLE.iter().zip(typed.roles.iter()) {
            if let Ok(Some(c)) = parse_field(l) {
                draft.light.insert(*role, c);
            }
            if let Ok(Some(c)) = parse_field(d) {
                draft.dark.insert(*role, c);
            }
        }
    }

    /// A field changed.
    fn changed(&self) {
        self.read_fields();
        self.fill_rows();
    }

    /// The draft becomes the theme at `index` of the list.
    fn start_from(&self, index: usize) {
        let Some(entry) = self.entries.borrow().get(index).cloned() else {
            return;
        };
        let mut theme = themes::load(&self.paths.themes, Some(&entry.id));
        theme.name = entry.name;
        *self.draft.borrow_mut() = theme;
        self.fill_fields();
        self.window.set_status(SharedString::new());
    }

    /// Re-reads the theme list and selects `id`.
    fn reload_list(&self, id: &str) {
        let entries = themes::list(&self.paths.themes, self.strings());
        self.window.set_themes(model(
            entries
                .iter()
                .map(|e| SharedString::from(e.name.as_str()))
                .collect(),
        ));
        let index = entries.iter().position(|e| e.id == id).unwrap_or(0);
        self.window
            .set_start_index(i32::try_from(index).unwrap_or(0));
        *self.entries.borrow_mut() = entries;
    }

    /// Saves the draft; with `use_it`, also makes it the theme of the settings. Says what
    /// happened in the status line.
    fn save(&self, use_it: bool) {
        self.read_fields();
        let strings = self.strings();
        let mut draft = self.draft.borrow().clone();
        draft.name =
            lastdrop_core::theme::clean_name(&draft.name, strings.template(Msg::StudioTitle));
        let saved = themes::save(&self.paths.themes, &draft).and_then(|id| {
            if use_it {
                use_theme(&self.paths, &id)?;
            }
            Ok(id)
        });
        let status = match saved {
            Ok(id) => {
                log::info!(
                    "theme studio: saved {id}.json{}",
                    if use_it { ", in use" } else { "" }
                );
                self.reload_list(&id);
                crate::settings_window::themes_changed(if use_it { Some(&id) } else { None });
                let msg = if use_it {
                    Msg::StudioUsed
                } else {
                    Msg::StudioSaved
                };
                text(strings, msg, &[("file", &format!("{id}.json"))])
            }
            Err(e) => {
                log::warn!("theme studio: not saved: {e}");
                text(strings, Msg::StudioNotSaved, &[("detail", &e.to_string())])
            }
        };
        self.window.set_status(status.into());
    }
}

/// Makes `id` the theme of `settings.json` ([`write_theme`]) and tells the tray process, as a
/// change in Settings does.
fn use_theme(paths: &Paths, id: &str) -> std::io::Result<()> {
    write_theme(&SettingsFile::new(paths.settings.clone()), id)?;
    dialog::notify(Changes {
        settings: true,
        ..Changes::default()
    });
    Ok(())
}

/// Makes `id` the theme of `file`: one read-merge-write under the settings lock (settings-spec
/// §6), so only the theme changes and what another writer wrote stays. Returns what was written.
fn write_theme(file: &SettingsFile, id: &str) -> std::io::Result<Settings> {
    file.update(&Settings::default(), |mut settings| {
        settings.theme = (id != lastdrop_core::theme::DEFAULT_ID).then(|| id.to_owned());
        settings
    })
}

/// Opens Theme Studio starting from the theme `start` (else the settings' theme), or brings the
/// open one to the front.
pub fn open(paths: &Paths, start: Option<&str>) -> Result<(), AppError> {
    let open = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = open {
        session.window.show()?;
        dialog::bring_to_front(session.window.as_weak());
        return Ok(());
    }
    let (settings, _) = persist::load_settings(&paths.settings);
    let wording = Wording::read(&settings);
    let id = start
        .map(str::to_owned)
        .or_else(|| settings.theme.clone())
        .unwrap_or_else(|| lastdrop_core::theme::DEFAULT_ID.to_owned());
    let window = wiring::create_window(false, ui::ThemeStudioWindow::new)?;
    build(&window, paths, wording, &id);
    if let Err(e) = window.show() {
        release();
        return Err(e.into());
    }
    dialog::bring_to_front(window.as_weak());
    Ok(())
}

/// Sets up `window` as Theme Studio starting from the theme `id`, in `wording`'s language, and
/// keeps it as the open window (`--preview-theme-studio` uses it too).
pub(crate) fn build(window: &ui::ThemeStudioWindow, paths: &Paths, wording: Wording, id: &str) {
    window.set_texts(texts(wording.strings()));
    let session = Rc::new(Session {
        window: window.clone_strong(),
        paths: paths.clone(),
        wording,
        entries: RefCell::new(Vec::new()),
        draft: RefCell::new(Theme::default()),
        typed: RefCell::new(Typed::default()),
        windows: lastdrop_win::theme::read(),
    });
    session.reload_list(id);
    let index = usize::try_from(window.get_start_index()).unwrap_or(0);
    session.start_from(index);
    window.on_start_changed(|i| with_session(|s| s.start_from(usize::try_from(i).unwrap_or(0))));
    window.on_changed(|| with_session(Session::changed));
    window.on_role_edited(|i, dark, value| {
        with_session(|s| {
            if let Some(row) = s
                .typed
                .borrow_mut()
                .roles
                .get_mut(usize::try_from(i).unwrap_or(usize::MAX))
            {
                if dark {
                    row.1 = value.to_string();
                } else {
                    row.0 = value.to_string();
                }
            }
            s.changed();
        });
    });
    window.on_widget_toggled(|i, shown| {
        with_session(|s| {
            if let Some(w) = s
                .draft
                .borrow_mut()
                .layout
                .widgets
                .get_mut(usize::try_from(i).unwrap_or(usize::MAX))
            {
                w.shown = shown;
            }
            s.fill_rows();
        });
    });
    window.on_widget_moved(|i, delta| {
        with_session(|s| {
            move_widget(&mut s.draft.borrow_mut().layout.widgets, i, delta);
            s.fill_rows();
        });
    });
    window.on_save(|| with_session(|s| s.save(false)));
    window.on_save_use(|| with_session(|s| s.save(true)));
    window.on_open_folder(|| {
        with_session(|s| {
            let dir: PathBuf = s.paths.themes.clone();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                log::warn!("theme studio: no themes folder: {e}");
                return;
            }
            if let Err(e) = lastdrop_win::shell::open_folder(&dir) {
                log::warn!("theme studio: themes folder not opened: {e}");
            }
        });
    });
    window.on_dismiss(close);
    window.window().on_close_requested(|| {
        close();
        slint::CloseRequestResponse::HideWindow
    });
    SESSION.with(|s| *s.borrow_mut() = Some(session));
}

/// Moves the header widget at `index` one header place up (`delta` −1) or down (+1), swapping it
/// with the header widget there; the other widgets keep their places.
fn move_widget(widgets: &mut [lastdrop_core::theme::Widget], index: i32, delta: i32) {
    let header: Vec<usize> = (0..widgets.len())
        .filter(|i| widgets[*i].id.in_header())
        .collect();
    let Some(place) = usize::try_from(index)
        .ok()
        .and_then(|i| header.iter().position(|h| *h == i))
    else {
        return;
    };
    let target = if delta < 0 {
        place.checked_sub(1)
    } else {
        Some(place + 1)
    };
    if let Some(other) = target.and_then(|t| header.get(t)) {
        widgets.swap(header[place], *other);
    }
}

/// Closes the window and releases it ([`dialog::release_later`]); a draft not saved is dropped
/// (Close, ✕, and the installer's [`dialog::CLOSE_REQUEST`]).
pub fn close() {
    let session = SESSION.with(|s| s.borrow_mut().take());
    if let Some(session) = session {
        let _ = session.window.hide();
        dialog::release_later(session);
    }
}

/// Drops the window (after the event loop, or when it could not be shown).
pub fn release() {
    drop(SESSION.with(|s| s.borrow_mut().take()));
}

#[cfg(test)]
mod tests {
    use lastdrop_core::theme::{Widget, id_for};

    use super::*;

    #[test]
    fn using_a_theme_waits_for_the_settings_lock_and_keeps_what_was_written_meanwhile() {
        use std::sync::mpsc;
        use std::time::Duration;

        use lastdrop_win::named_lock::NamedLock;

        let dir = std::env::temp_dir().join(format!(
            "lastdrop-studio-test-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = SettingsFile::with_lock(
            dir.join("settings.json"),
            &format!(r"Local\Lastdrop.Test.Studio.{}", std::process::id()),
        );
        persist::save_settings(&file.path, &Settings::default()).expect("seed");

        // Another writer holds the lock for its read-merge-write.
        let (locked_tx, locked) = mpsc::channel();
        let (release, until) = mpsc::channel::<()>();
        let other = file.clone();
        let holder = std::thread::spawn(move || {
            let lock = NamedLock::acquire(&other.lock, Duration::from_secs(5)).expect("lock");
            locked_tx.send(()).expect("locked");
            let _ = until.recv();
            let (mut settings, _) = persist::load_settings(&other.path);
            settings.readout.enabled = false;
            persist::save_settings(&other.path, &settings).expect("write");
            drop(lock);
        });
        locked.recv().expect("locked");
        let studio_file = file.clone();
        let studio = std::thread::spawn(move || write_theme(&studio_file, "ember"));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            persist::load_settings(&file.path).0.theme,
            None,
            "Theme Studio waits for the lock"
        );
        release.send(()).expect("release");
        holder.join().expect("holder");
        let written = studio.join().expect("studio").expect("written");
        assert_eq!(written.theme.as_deref(), Some("ember"));
        assert!(!written.readout.enabled, "the other writer's change stays");
        assert_eq!(persist::load_settings(&file.path).0, written);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn header_widgets_move_among_themselves() {
        let mut widgets: Vec<Widget> = WidgetId::ALL
            .iter()
            .map(|&id| Widget { id, shown: true })
            .collect();
        // Sparkline (index 2) up: it swaps with Age.
        move_widget(&mut widgets, 2, -1);
        let ids: Vec<WidgetId> = widgets.iter().map(|w| w.id).collect();
        assert_eq!(
            &ids[..3],
            [WidgetId::Plan, WidgetId::Sparkline, WidgetId::Age]
        );
        // The first cannot go up, the last header widget cannot go down, body widgets never move.
        move_widget(&mut widgets, 0, -1);
        move_widget(&mut widgets, 2, 1);
        move_widget(&mut widgets, 4, -1);
        let again: Vec<WidgetId> = widgets.iter().map(|w| w.id).collect();
        assert_eq!(again, ids);
        assert_eq!(parse_field(""), Ok(None));
        assert_eq!(parse_field("#123"), Err(()));
        assert_eq!(id_for("Ember"), "ember-2");
    }
}
