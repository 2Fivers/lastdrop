//! `--preview-flyout <png>` and `--preview-hovercard <png>` (with `--demo`, `--scale <s>`,
//! `--dark`): render the flyout and the hover card without a window and save them as PNGs, so the
//! UI can be compared with `docs/design/final/flyout-100.png` and `readout-100.png` §5.
//! `--preview-settings <png>` and `--preview-about <png>` (with `--demo` only) and
//! `--preview-history <png>` (the demo's history, or without `--demo` the user's) render the
//! dialog process's windows the same way. The settings window renders each page's viewport, and a
//! few states, at its preferred and its smallest size into `<stem>-<page>-<w>x<h>.png`
//! (settings-spec §9); `--contrast` renders it in a contrast theme.
//!
//! The render uses Slint's software renderer through a headless platform
//! (`MinimalSoftwareWindow`s): no window is created, nothing appears on the desktop and nothing
//! takes focus. The window's backdrop is the DWM solid fallback of the mode (`#F9F9F9` /
//! `#2C2C2C`, F§7: "transparency off"); DWM's corners, border and shadow are not part of it.
//!
//! With `--demo` the data is [`crate::demo`]'s: invented, no network, no files, no Windows
//! settings. Without it, every enabled service is read once with the real providers and the
//! user's theme, format, language and history. `--language <tag>` (`en`, `sv`) renders in that
//! language instead (with `--demo`, in its regional format too).

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use lastdrop_core::consts::layout::{CARD_W, FLYOUT_MAX_H, FLYOUT_W};
use lastdrop_core::flyout::{FlyoutModel, build_flyout};
use lastdrop_core::grammar::{Language, Strings};
use lastdrop_core::model::{MeterKey, MeterNames};
use lastdrop_core::readout::BuildContext;
use lastdrop_core::readout::hovercard::{HoverAnchor, HoverCardModel, build_hover_card};
use lastdrop_core::settings::ReadoutContent;
use lastdrop_providers::FetchContext;
use lastdrop_win::theme::ThemeInfo;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};

use crate::history::HistoryStore;
use crate::scheduler::{Job, WorkerIo, run_job};
use crate::settings_window::Wording;
use crate::state::AppState;
use crate::wiring::{self, CardMenu};
use crate::writer::Writer;
use crate::{AppError, Args, demo, persist, png, ui};

/// The hover card's tallest preview (it is as tall as its content; R§8.1 fits it on screen).
const CARD_MAX_PREVIEW_H: f32 = 2000.0;
/// The settings window's preview sizes, logical px: its preferred size and its smallest.
const SETTINGS_SIZES: [(f32, f32); 2] = [
    crate::settings_window::PREFERRED_SIZE,
    crate::settings_window::MIN_SIZE,
];
/// The about window's preferred width.
const ABOUT_PREVIEW_W: f32 = 420.0;
/// The usage history window's preview size: its preferred width, and at most this tall (it is as
/// tall as its page).
const HISTORY_PREVIEW_W: f32 = 640.0;
/// Theme Studio's preview size, logical px.
const STUDIO_PREVIEW_W: f32 = 880.0;
/// See [`STUDIO_PREVIEW_W`].
const STUDIO_PREVIEW_H: f32 = 1100.0;
/// See [`HISTORY_PREVIEW_W`].
const HISTORY_PREVIEW_H: f32 = 1500.0;

/// The headless platform: software-rendered windows without an OS window or an event loop.
struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// The DWM solid fallback behind the popups with transparency off (F§7, R§7).
pub(crate) fn solid_backdrop(dark: bool) -> slint::Color {
    if dark {
        slint::Color::from_rgb_u8(0x2C, 0x2C, 0x2C)
    } else {
        slint::Color::from_rgb_u8(0xF9, 0xF9, 0xF9)
    }
}

/// Renders what `args` asks for (see the module docs): the flyout to `args.preview_flyout`,
/// the hover card to `args.preview_hover_card`.
pub fn run(args: &Args) -> Result<(), AppError> {
    if args.preview_flyout.is_none()
        && args.preview_hover_card.is_none()
        && args.preview_settings.is_none()
        && args.preview_about.is_none()
        && args.preview_history.is_none()
        && args.preview_theme_studio.is_none()
    {
        return Ok(());
    }
    let scale = args.scale.unwrap_or(1.0);
    slint::platform::set_platform(Box::new(Headless)).map_err(|e| AppError::Ui(e.to_string()))?;
    if let Some(out) = args.preview_theme_studio.as_deref() {
        render_theme_studio(args, scale, out)?;
    }
    if args.demo {
        let scenario = args
            .demo_scenario
            .as_deref()
            .and_then(demo::Scenario::parse)
            .unwrap_or_default();
        let language = args
            .language
            .as_deref()
            .and_then(Language::from_tag)
            .unwrap_or_default();
        let mut world = demo::Demo::scenario(args.dark, scenario).in_language(language);
        if let Some(id) = args.theme.as_deref() {
            let paths = persist::Paths::from_env()?;
            let theme = crate::themes::load(&paths.themes, Some(id));
            world.theme = lastdrop_win::theme::apply(world.theme, &theme);
        }
        let strings = language.strings();
        if let Some(out) = args.preview_flyout.as_deref() {
            let model = world.flyout();
            let menus = demo_menus(&model, strings);
            render_flyout(&model, &menus, strings, &world.theme, scale, out)?;
        }
        if let Some(out) = args.preview_hover_card.as_deref() {
            render_hover_card(&world.hover_card(), &world.theme, scale, out)?;
        }
        if let Some(out) = args.preview_settings.as_deref() {
            render_settings(&world, args.contrast, scale, out)?;
        }
        if let Some(out) = args.preview_about.as_deref() {
            render_about(strings, args.dark, scale, out)?;
        }
        if let Some(out) = args.preview_history.as_deref() {
            render_history(&world, scale, out)?;
        }
        return Ok(());
    }
    let live = live_models(
        args.language.as_deref().and_then(Language::from_tag),
        args.theme.as_deref(),
    )?;
    if let Some(out) = args.preview_flyout.as_deref() {
        render_flyout(
            &live.flyout,
            &live.menus,
            live.strings,
            &live.theme,
            scale,
            out,
        )?;
    }
    if let Some(out) = args.preview_hover_card.as_deref() {
        render_hover_card(&live.hover_card, &live.theme, scale, out)?;
    }
    if let Some(out) = args.preview_history.as_deref() {
        render_live_history(
            args.language.as_deref().and_then(Language::from_tag),
            scale,
            out,
        )?;
    }
    Ok(())
}

/// Renders the usage history window with the user's history: the first service that has any,
/// over the last 30 days, in `language` (else the settings' and Windows').
fn render_live_history(language: Option<Language>, scale: f64, out: &Path) -> Result<(), AppError> {
    use crate::history_window::{build, fill_choices, fill_model, sources, with_history};
    let paths = persist::Paths::from_env()?;
    crate::custom_services::load(&paths.providers);
    let (settings, _) = persist::load_settings(&paths.settings);
    let mut wording = Wording::read(&settings);
    if let Some(language) = language {
        wording.language = language;
    }
    let strings = wording.strings();
    let all = sources(&settings, &persist::load_state(&paths.state), strings);
    let found = with_history(all, &paths.history);
    let now = Timestamp::now();
    let history: std::collections::BTreeMap<MeterKey, Vec<lastdrop_core::stats::Sample>> = found
        .first()
        .map(|s| {
            s.meters
                .iter()
                .map(|m| {
                    let samples = crate::history::read_meter(&paths.history, &m.key, now);
                    (m.key.clone(), samples)
                })
                .collect()
        })
        .unwrap_or_default();
    let window = ui::HistoryWindow::new()?;
    fill_choices(&window, &found, strings);
    window.set_service_index(0);
    window.set_range_index(1);
    let model = build(
        found.first(),
        &history,
        lastdrop_core::charts::Range::Month,
        now,
        &wording.locale,
        strings,
    );
    fill_model(&window, &model);
    let folders = crate::history_window::log_folders(&settings);
    if let Some((source, folder)) = found
        .first()
        .and_then(|s| folders.get(&s.key).map(|f| (s, f)))
    {
        let since = now
            .checked_sub(jiff::SignedDuration::from_hours(24 * 31))
            .unwrap_or(now);
        let started = std::time::Instant::now();
        let fs = lastdrop_providers::real::RealFs;
        let log = match source.key.provider {
            lastdrop_core::model::ProviderId::Claude => {
                lastdrop_providers::tokens::read_claude(&fs, folder, since)
            }
            _ => lastdrop_providers::tokens::read_codex(&fs, folder, since),
        };
        log::info!(
            "preview: {} session logs read in {} ms: {} requests",
            log.files,
            started.elapsed().as_millis(),
            log.requests.len()
        );
        let section = lastdrop_core::tokens::build(&lastdrop_core::tokens::TokensInput {
            log: &log,
            prices: &crate::history_window::load_prices(&paths.prices),
            client: source.key.provider.client_name().unwrap_or_default(),
            range: lastdrop_core::charts::Range::Month,
            now,
            locale: &wording.locale,
            strings,
        });
        crate::history_window::fill_tokens(&window, Some(&section));
    }
    window.show()?;
    snapshot_to_png(
        window.window(),
        scale,
        HISTORY_PREVIEW_W,
        HISTORY_PREVIEW_H,
        &|| window.get_content_height(),
        &|_| {},
        out,
    )?;
    window.hide()?;
    Ok(())
}

/// Card menus for the demo (every service has a usage page, nothing pinned).
fn demo_menus(model: &FlyoutModel, strings: &dyn Strings) -> Vec<CardMenu> {
    model
        .cards
        .iter()
        .map(|card| wiring::card_menu(card, strings, &[], true))
        .collect()
}

/// A logical length at `scale` in whole device px (half up).
fn device(v: f32, scale: f64) -> u32 {
    let d = (f64::from(v) * scale + 0.5).floor();
    // Window sizes are a few thousand px at most.
    d.clamp(1.0, 16_384.0) as u32
}

/// Sizes a shown headless window to `width` × the content height `measure` reports (the
/// component must be shown first: its repeaters are instantiated then), renders it and writes the
/// PNG.
fn snapshot_to_png(
    window: &slint::Window,
    scale: f64,
    width: f32,
    max_height: f32,
    measure: &dyn Fn() -> f32,
    set_height: &dyn Fn(f32),
    out: &Path,
) -> Result<(), AppError> {
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        // The scale is 0.5..=4.
        scale_factor: scale as f32,
    });
    window.set_size(slint::LogicalSize::new(width, max_height));
    let height = measure().min(max_height);
    set_height(height);
    window.set_size(slint::PhysicalSize::new(
        device(width, scale),
        device(height, scale),
    ));
    let snapshot = window.take_snapshot()?;
    png::write_rgba(
        out,
        snapshot.width(),
        snapshot.height(),
        snapshot.as_bytes(),
    )?;
    Ok(())
}

/// Renders the flyout `model` at `scale` with `theme` and the fixed labels of `strings`, and
/// writes the PNG.
pub fn render_flyout(
    model: &FlyoutModel,
    menus: &[CardMenu],
    strings: &dyn Strings,
    theme: &ThemeInfo,
    scale: f64,
    out: &Path,
) -> Result<(), AppError> {
    let flyout = ui::Flyout::new()?;
    let global = flyout.global::<ui::Theme>();
    wiring::set_theme(&global, theme);
    global.set_solid_backdrop(solid_backdrop(theme.taskbar_dark));
    wiring::set_flyout_labels(&flyout, strings);
    wiring::set_flyout(&flyout, model, scale, menus);
    flyout.show()?;
    snapshot_to_png(
        flyout.window(),
        scale,
        FLYOUT_W,
        FLYOUT_MAX_H,
        &|| flyout.get_content_height(),
        &|h| flyout.set_flyout_height(h),
        out,
    )?;
    flyout.hide()?;
    Ok(())
}

/// Renders the hover card `model` at `scale` with `theme` and writes the PNG.
pub fn render_hover_card(
    model: &HoverCardModel,
    theme: &ThemeInfo,
    scale: f64,
    out: &Path,
) -> Result<(), AppError> {
    let card = ui::HoverCard::new()?;
    let global = card.global::<ui::Theme>();
    wiring::set_theme(&global, theme);
    global.set_solid_backdrop(solid_backdrop(theme.taskbar_dark));
    wiring::set_hover_card(&card, model, scale);
    card.show()?;
    snapshot_to_png(
        card.window(),
        scale,
        CARD_W,
        CARD_MAX_PREVIEW_H,
        &|| card.get_content_height(),
        &|h| card.set_card_height(h),
        out,
    )?;
    card.hide()?;
    Ok(())
}

/// What a settings window preview shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsView {
    /// A page, as it opens.
    Page(i32),
    /// The first run's deep link: the accounts with the found folders.
    Accounts,
    /// Services with Meshy's and OpenCode Go's API keys expanded.
    Keys,
    /// The tray menu's "Pinned meters…": the readout's pins, choosing.
    Pins,
    /// General with a refused shortcut, and the InfoBar of a write that failed on closing.
    Errors,
    /// General scrolled to its end: the shortcut on (Ctrl + Alt + L) and Check now's answer, a
    /// newer version.
    Updates,
}

/// The settings window's previews: each page, then the states, at both sizes.
const SETTINGS_VIEWS: [(&str, SettingsView); 9] = [
    (
        "services",
        SettingsView::Page(crate::settings_window::PAGE_SERVICES),
    ),
    (
        "display",
        SettingsView::Page(crate::settings_window::PAGE_DISPLAY),
    ),
    (
        "notifications",
        SettingsView::Page(crate::settings_window::PAGE_NOTIFICATIONS),
    ),
    (
        "general",
        SettingsView::Page(crate::settings_window::PAGE_GENERAL),
    ),
    ("accounts", SettingsView::Accounts),
    ("keys", SettingsView::Keys),
    ("pins", SettingsView::Pins),
    ("errors", SettingsView::Errors),
    ("updates", SettingsView::Updates),
];

/// Windows' "Night sky" contrast colours (the `--contrast` preview).
fn night_sky() -> lastdrop_win::theme::HighContrastColors {
    let rgb = |r, g, b| lastdrop_win::palette::Argb::new(0xFF, r, g, b);
    lastdrop_win::theme::HighContrastColors {
        window_text: rgb(0xFF, 0xFF, 0xFF),
        window: rgb(0x00, 0x00, 0x00),
        highlight: rgb(0xD6, 0xB4, 0xFD),
        highlight_text: rgb(0x2B, 0x2B, 0x2B),
        gray_text: rgb(0xA6, 0xA6, 0xA6),
        hotlight: rgb(0x80, 0x80, 0xFF),
    }
}

/// What the settings window shows for the demo: its settings and invented accounts
/// ([`demo::Demo::accounts`]) with the described services this process has installed (turned off,
/// as loading `settings.json` adds them), the meters its services report, a stored Meshy key, in
/// the demo's language and regional format (Windows' display language is the demo's language
/// too), and no files: the themes are the built-in ones.
pub fn demo_settings_inputs(world: &demo::Demo, contrast: bool) -> crate::settings_window::Inputs {
    use crate::settings_window::ListedMeter;
    let reported: Vec<ListedMeter> = world
        .services
        .iter()
        .flat_map(|s| {
            let listed = move |id: &lastdrop_core::model::MeterId, names: &MeterNames, balance| {
                ListedMeter {
                    key: MeterKey {
                        provider: s.snapshot.provider,
                        account: s.account.key.clone(),
                        meter: id.clone(),
                    },
                    names: names.clone(),
                    balance,
                }
            };
            let meters = s
                .snapshot
                .meters
                .iter()
                .map(move |m| listed(&m.id, &m.names, false));
            let balances = s
                .snapshot
                .balances
                .iter()
                .map(move |b| listed(&b.id, &b.names, true));
            meters.chain(balances)
        })
        .collect();
    let (mut settings, accounts) = world.accounts();
    settings.add_new_providers();
    let mut theme = world.theme;
    theme.apps_dark = theme.taskbar_dark;
    if contrast {
        theme.high_contrast = Some(night_sky());
    }
    crate::settings_window::Inputs {
        settings,
        reported,
        accounts,
        keys: vec![
            (lastdrop_core::model::ProviderId::Meshy, true),
            (lastdrop_core::model::ProviderId::OpenCodeGo, false),
        ],
        wording: Wording {
            language: world.language,
            ui_languages: vec![world.language.tag().to_owned()],
            locale: world.locale.clone(),
        },
        theme,
        paths: persist::Paths::under(
            r"C:\Users\tester\AppData\Roaming",
            r"C:\Users\tester\AppData\Local",
        ),
    }
}

/// Renders the settings window for the demo ([`demo_settings_inputs`]): every page and state of
/// [`SETTINGS_VIEWS`] at both [`SETTINGS_SIZES`], next to `out` as `<stem>-<view>-<w>x<h>.png`.
fn render_settings(
    world: &demo::Demo,
    contrast: bool,
    scale: f64,
    out: &Path,
) -> Result<(), AppError> {
    use crate::settings_edits::MemoryOutside;
    use crate::settings_window::{self as sw, Focus};
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings".to_owned());
    let dir = out.parent().unwrap_or_else(|| Path::new("."));
    // The service list shows the described services Lastdrop ships, as on a fresh install.
    crate::custom_services::load_bundled();
    for (name, view) in SETTINGS_VIEWS {
        for (w, h) in SETTINGS_SIZES {
            let inputs = demo_settings_inputs(world, contrast);
            let mut outside = MemoryOutside {
                disk: inputs.settings.clone(),
                ..MemoryOutside::default()
            };
            if view == SettingsView::Errors {
                outside.fail_write = Some("The disk is full.".to_owned());
                outside.refuse_hotkey = Some(lastdrop_win::hotkey::HotkeyError::InUse);
            }
            let window = sw::start(inputs, Box::new(outside), false)?;
            if world.theme.taskbar_dark && !contrast {
                window.invoke_use_dark_scheme();
            }
            window.show()?;
            window.window().set_size(slint::LogicalSize::new(w, h));
            match view {
                SettingsView::Page(page) => window.set_page(page),
                SettingsView::Accounts => sw::reveal(Focus::Accounts),
                SettingsView::Keys => {
                    let rows: Vec<ui::ProviderRow> =
                        slint::Model::iter(&window.get_providers()).collect();
                    for (i, row) in rows.iter().enumerate() {
                        if row.id == "meshy" || row.id == "opencode_go" {
                            window.invoke_service_toggled(i32::try_from(i).unwrap_or(0), true);
                        }
                    }
                }
                SettingsView::Pins => sw::reveal(Focus::Pins),
                SettingsView::Errors => {
                    window.set_page(sw::PAGE_GENERAL);
                    window.set_hotkey_enabled(true);
                    window.set_hotkey_key("K".into());
                    window.invoke_hotkey_edited();
                    window.set_interval_index(1);
                    window.invoke_interval_selected(1);
                    // Closing commits at once: the shortcut is refused, the write fails.
                    window.invoke_dismiss();
                }
                SettingsView::Updates => {
                    window.set_hotkey_enabled(true);
                    window.invoke_hotkey_edited();
                    window.invoke_check_updates();
                    let mut newer = lastdrop_core::update::Version::current();
                    newer.minor += 1;
                    newer.patch = 0;
                    sw::check_answered(crate::dialog::CheckResult::available(&newer));
                    // Laid out first, so the end is where the shortcut's row has put it.
                    let _ = window.window().take_snapshot();
                    window.invoke_show_general_end();
                }
            }
            slint::platform::update_timers_and_animations();
            let file = dir.join(format!("{stem}-{name}-{w}x{h}.png"));
            snapshot_to_png(window.window(), scale, w, h, &|| h, &|_| {}, &file)?;
            window.hide()?;
            sw::release();
        }
    }
    Ok(())
}

/// The demo's services as the usage history window lists them: each with the meters and
/// balances of its reading.
fn demo_sources(world: &demo::Demo) -> Vec<crate::history_window::Source> {
    use lastdrop_core::charts::ChartMeter;
    use lastdrop_core::flyout::SparkKind;
    world
        .services
        .iter()
        .map(|s| {
            let key = |id: &lastdrop_core::model::MeterId| MeterKey {
                provider: s.snapshot.provider,
                account: s.account.key.clone(),
                meter: id.clone(),
            };
            let windows = s.snapshot.meters.iter().map(|m| ChartMeter {
                key: key(&m.id),
                names: m.names.clone(),
                kind: SparkKind::Window,
                window: Some(m.window),
                unit: None,
            });
            let balances = s.snapshot.balances.iter().map(|b| ChartMeter {
                key: key(&b.id),
                names: b.names.clone(),
                kind: SparkKind::Balance,
                window: None,
                unit: Some(b.unit.clone()),
            });
            crate::history_window::Source {
                key: lastdrop_core::model::SourceKey {
                    provider: s.snapshot.provider,
                    account: s.account.key.clone(),
                },
                label: s.snapshot.provider.display_name().to_owned(),
                meters: windows.chain(balances).collect(),
            }
        })
        .collect()
}

/// Renders the usage history window with the demo's first service over the last 30 days, in the
/// demo's colour mode.
fn render_history(world: &demo::Demo, scale: f64, out: &Path) -> Result<(), AppError> {
    use crate::history_window::{build, fill_choices, fill_model};
    let window = ui::HistoryWindow::new()?;
    if world.theme.apps_dark {
        window.invoke_use_dark_scheme();
    }
    let strings = world.language.strings();
    let sources = demo_sources(world);
    fill_choices(&window, &sources, strings);
    window.set_service_index(0);
    window.set_range_index(1);
    let model = build(
        sources.first(),
        &world.history,
        lastdrop_core::charts::Range::Month,
        world.now,
        &world.locale,
        strings,
    );
    fill_model(&window, &model);
    let first_is_claude = sources
        .first()
        .is_some_and(|s| s.key.provider == lastdrop_core::model::ProviderId::Claude);
    if first_is_claude {
        let log = demo::claude_tokens(world.now);
        let section = lastdrop_core::tokens::build(&lastdrop_core::tokens::TokensInput {
            log: &log,
            prices: &lastdrop_core::tokens::PriceTable::bundled(),
            client: "Claude Code",
            range: lastdrop_core::charts::Range::Month,
            now: world.now,
            locale: &world.locale,
            strings,
        });
        crate::history_window::fill_tokens(&window, Some(&section));
    }
    window.show()?;
    snapshot_to_png(
        window.window(),
        scale,
        HISTORY_PREVIEW_W,
        HISTORY_PREVIEW_H,
        &|| window.get_content_height(),
        &|_| {},
        out,
    )?;
    window.hide()?;
    Ok(())
}

/// Renders Theme Studio starting from `--theme` (else the settings' theme), in `--language`
/// (else the settings' and Windows').
fn render_theme_studio(args: &Args, scale: f64, out: &Path) -> Result<(), AppError> {
    let paths = persist::Paths::from_env()?;
    let (settings, _) = persist::load_settings(&paths.settings);
    let mut wording = Wording::read(&settings);
    if let Some(language) = args.language.as_deref().and_then(Language::from_tag) {
        wording.language = language;
    }
    let id = args
        .theme
        .clone()
        .or(settings.theme)
        .unwrap_or_else(|| lastdrop_core::theme::DEFAULT_ID.to_owned());
    let window = ui::ThemeStudioWindow::new()?;
    crate::theme_studio::build(&window, &paths, wording, &id);
    window.show()?;
    let result = snapshot_to_png(
        window.window(),
        scale,
        STUDIO_PREVIEW_W,
        STUDIO_PREVIEW_H,
        &|| STUDIO_PREVIEW_H,
        &|_| {},
        out,
    );
    window.hide()?;
    crate::theme_studio::release();
    result
}

/// Renders the about window in the language of `strings`.
fn render_about(strings: &dyn Strings, dark: bool, scale: f64, out: &Path) -> Result<(), AppError> {
    let window = ui::AboutWindow::new()?;
    if dark {
        window.invoke_use_dark_scheme();
    }
    window.set_texts(crate::dialog::about_texts(strings));
    window.show()?;
    let height = window.get_content_height();
    snapshot_to_png(
        window.window(),
        scale,
        ABOUT_PREVIEW_W,
        height,
        &|| height,
        &|_| {},
        out,
    )?;
    window.hide()?;
    Ok(())
}

/// What the live preview renders.
struct LiveModels {
    flyout: FlyoutModel,
    menus: Vec<CardMenu>,
    hover_card: HoverCardModel,
    theme: ThemeInfo,
    strings: &'static dyn Strings,
}

/// The flyout and the hover card from one synchronous read of every enabled service with the
/// real providers, the user's theme, regional format and history, in `language` (else the
/// language of the settings and Windows).
fn live_models(language: Option<Language>, theme: Option<&str>) -> Result<LiveModels, AppError> {
    // COM for UISettings (theme) and Network List Manager (after a failed read).
    // SAFETY: called once on this thread before any COM object is created on it.
    let _ = unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        )
    };
    let paths = persist::Paths::from_env()?;
    let (settings, _) = persist::load_settings(&paths.settings);
    let theme_data = crate::themes::load(&paths.themes, theme.or(settings.theme.as_deref()));
    let theme = lastdrop_win::theme::apply(lastdrop_win::theme::read(), &theme_data);
    let locale = lastdrop_win::locale::read(TimeZone::system());
    let now = Timestamp::now();
    // History writes of this read go through a writer of its own, drained when it drops at the
    // end of this function (after `state`).
    let writer = Writer::start()?;
    let history = HistoryStore::open(&paths.history, now, writer.handle())?;
    let mut state = AppState::new(
        settings,
        history,
        theme,
        locale,
        lastdrop_win::locale::ui_languages(),
    );
    if let Some(language) = language {
        state.strings = language.strings();
    }
    // The meters earlier runs saw keep their room, as in the app (read only: a preview never
    // rewrites the state file).
    state.restore_saved(&persist::load_state(&paths.state), now);

    let io = Arc::new(WorkerIo::new());
    let ctx = FetchContext {
        clock: &io.clock,
        http: &io.http,
        fs: &io.fs,
        secrets: &io.secrets,
        spawner: &io.spawner,
        env: &io.env,
        deadline: now,
        manual: false,
    };
    let jobs: Vec<Job> = state
        .sources
        .iter()
        .map(|(key, source)| Job {
            source: key.clone(),
            provider: source.provider.clone(),
            account: source.source.clone(),
            manual: true,
            probe: None,
            identity: crate::scheduler::IdentityRead::Always,
        })
        .collect();
    for job in &jobs {
        let result = run_job(
            job,
            &io.clock,
            &ctx,
            &lastdrop_win::network::connected_to_internet,
        );
        state.apply_result(result);
    }
    let now = Timestamp::now();
    state.evaluate(now);
    let eval = state
        .evaluation
        .as_ref()
        .ok_or_else(|| AppError::Ui("no evaluation".to_owned()))?;
    let cx = BuildContext {
        text: state.text_context(now),
        settings: &state.settings,
    };
    let flyout = build_flyout(eval, &state.history, false, &cx);
    let hover_card = build_hover_card(
        eval,
        HoverAnchor::TrayIcon {
            readout_visible: false,
        },
        &[],
        &cx,
    );
    let pins = match &state.settings.readout.content {
        ReadoutContent::Pinned(p) => p.clone(),
        ReadoutContent::Auto => Vec::new(),
    };
    let menus = flyout
        .cards
        .iter()
        .map(|card| {
            let usage = state
                .sources
                .get(&card.source)
                .and_then(|s| s.provider.usage_page())
                .is_some();
            wiring::card_menu(card, state.strings, &pins, usage)
        })
        .collect();
    Ok(LiveModels {
        flyout,
        menus,
        hover_card,
        theme,
        strings: state.strings,
    })
}
