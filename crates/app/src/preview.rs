//! `--preview-flyout <png>` and `--preview-hovercard <png>` (with `--demo`, `--scale <s>`,
//! `--dark`): render the flyout and the hover card without a window and save them as PNGs, so the
//! UI can be compared with `docs/design/final/flyout-100.png` and `readout-100.png` §5.
//! `--preview-settings <png>` and `--preview-about <png>` (with `--demo` only) and
//! `--preview-history <png>` (the demo's history, or without `--demo` the user's) render the
//! dialog process's windows the same way.
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
/// The settings window's preview size: its preferred width, tall enough for the whole page.
const SETTINGS_PREVIEW_W: f32 = 480.0;
/// See [`SETTINGS_PREVIEW_W`].
const SETTINGS_PREVIEW_H: f32 = 1500.0;
/// The about window's preferred width.
const ABOUT_PREVIEW_W: f32 = 420.0;
/// The usage history window's preview size: its preferred width, tall enough for the page.
const HISTORY_PREVIEW_W: f32 = 640.0;
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
fn solid_backdrop(dark: bool) -> slint::Color {
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
    {
        return Ok(());
    }
    let scale = args.scale.unwrap_or(1.0);
    slint::platform::set_platform(Box::new(Headless)).map_err(|e| AppError::Ui(e.to_string()))?;
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
        let world = demo::Demo::scenario(args.dark, scenario).in_language(language);
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
            render_settings(&world, scale, out)?;
        }
        if let Some(out) = args.preview_about.as_deref() {
            render_about(strings, scale, out)?;
        }
        if let Some(out) = args.preview_history.as_deref() {
            render_history(&world, scale, out)?;
        }
        return Ok(());
    }
    let live = live_models(args.language.as_deref().and_then(Language::from_tag))?;
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
        &|| HISTORY_PREVIEW_H,
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

/// Renders the settings window with the demo's settings and invented accounts
/// ([`demo::Demo::accounts`]), in the demo's language and regional format (Windows' display
/// language is the demo's language too).
fn render_settings(world: &demo::Demo, scale: f64, out: &Path) -> Result<(), AppError> {
    let window = ui::SettingsWindow::new()?;
    let reported: Vec<(MeterKey, MeterNames)> = world
        .services
        .iter()
        .flat_map(|s| {
            s.snapshot.meters.iter().map(move |m| {
                (
                    MeterKey {
                        provider: s.snapshot.provider,
                        account: s.account.key.clone(),
                        meter: m.id.clone(),
                    },
                    m.names.clone(),
                )
            })
        })
        .collect();
    let wording = Wording {
        language: world.language,
        ui_languages: vec![world.language.tag().to_owned()],
        locale: world.locale.clone(),
    };
    let (settings, accounts) = world.accounts();
    let duplicates =
        crate::settings_accounts::meter_accounts(&settings, &accounts, wording.strings())
            .duplicates;
    let draft = crate::settings_window::draft_with(settings, &reported, &duplicates);
    let keys = [
        (lastdrop_core::model::ProviderId::Meshy, true),
        (lastdrop_core::model::ProviderId::OpenCodeGo, false),
    ];
    crate::settings_window::fill(&window, &draft, &reported, &keys, &wording, &accounts);
    window.show()?;
    snapshot_to_png(
        window.window(),
        scale,
        SETTINGS_PREVIEW_W,
        SETTINGS_PREVIEW_H,
        &|| SETTINGS_PREVIEW_H,
        &|_| {},
        out,
    )?;
    window.hide()?;
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

/// Renders the usage history window with the demo's first service over the last 30 days.
fn render_history(world: &demo::Demo, scale: f64, out: &Path) -> Result<(), AppError> {
    use crate::history_window::{build, fill_choices, fill_model};
    let window = ui::HistoryWindow::new()?;
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
        &|| HISTORY_PREVIEW_H,
        &|_| {},
        out,
    )?;
    window.hide()?;
    Ok(())
}

/// Renders the about window in the language of `strings`.
fn render_about(strings: &dyn Strings, scale: f64, out: &Path) -> Result<(), AppError> {
    let window = ui::AboutWindow::new()?;
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
fn live_models(language: Option<Language>) -> Result<LiveModels, AppError> {
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
    let theme = lastdrop_win::theme::read();
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
