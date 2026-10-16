//! `--preview-flyout <png>` and `--preview-hovercard <png>` (with `--demo`, `--scale <s>`,
//! `--dark`): render the flyout and the hover card without a window and save them as PNGs, so the
//! UI can be compared with `docs/design/final/flyout-100.png` and `readout-100.png` §5.
//!
//! The render uses Slint's software renderer through a headless platform
//! (`MinimalSoftwareWindow`s): no window is created, nothing appears on the desktop and nothing
//! takes focus. The window's backdrop is the DWM solid fallback of the mode (`#F9F9F9` /
//! `#2C2C2C`, F§7: "transparency off"); DWM's corners, border and shadow are not part of it.
//!
//! With `--demo` the data is [`crate::demo`]'s: invented, no network, no files, no Windows
//! settings. Without it, every enabled service is read once with the real providers and the
//! user's theme, format and history.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use lastdrop_core::consts::layout::{CARD_W, FLYOUT_MAX_H, FLYOUT_W};
use lastdrop_core::flyout::{FlyoutModel, build_flyout};
use lastdrop_core::grammar::English;
use lastdrop_core::model::MeterKey;
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
use crate::state::AppState;
use crate::wiring::{self, CardMenu};
use crate::writer::Writer;
use crate::{AppError, Args, demo, persist, png, ui};

/// The hover card's tallest preview (it is as tall as its content; R§8.1 fits it on screen).
const CARD_MAX_PREVIEW_H: f32 = 2000.0;
/// The settings window's preview size: its preferred width, tall enough for the whole page.
const SETTINGS_PREVIEW_W: f32 = 480.0;
/// See [`SETTINGS_PREVIEW_W`].
const SETTINGS_PREVIEW_H: f32 = 900.0;
/// The about window's preferred width.
const ABOUT_PREVIEW_W: f32 = 420.0;

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
        let world = demo::Demo::scenario(args.dark, scenario);
        if let Some(out) = args.preview_flyout.as_deref() {
            let model = world.flyout();
            render_flyout(&model, &demo_menus(&model), &world.theme, scale, out)?;
        }
        if let Some(out) = args.preview_hover_card.as_deref() {
            render_hover_card(&world.hover_card(), &world.theme, scale, out)?;
        }
        if let Some(out) = args.preview_settings.as_deref() {
            render_settings(&world, scale, out)?;
        }
        if let Some(out) = args.preview_about.as_deref() {
            render_about(scale, out)?;
        }
        return Ok(());
    }
    let live = live_models()?;
    if let Some(out) = args.preview_flyout.as_deref() {
        render_flyout(&live.flyout, &live.menus, &live.theme, scale, out)?;
    }
    if let Some(out) = args.preview_hover_card.as_deref() {
        render_hover_card(&live.hover_card, &live.theme, scale, out)?;
    }
    Ok(())
}

/// Card menus for the demo (every service has a usage page, nothing pinned).
fn demo_menus(model: &FlyoutModel) -> Vec<CardMenu> {
    model
        .cards
        .iter()
        .map(|card| wiring::card_menu(card, &English, &[], true))
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

/// Renders the flyout `model` at `scale` with `theme` and writes the PNG.
pub fn render_flyout(
    model: &FlyoutModel,
    menus: &[CardMenu],
    theme: &ThemeInfo,
    scale: f64,
    out: &Path,
) -> Result<(), AppError> {
    let flyout = ui::Flyout::new()?;
    let global = flyout.global::<ui::Theme>();
    wiring::set_theme(&global, theme);
    global.set_solid_backdrop(solid_backdrop(theme.taskbar_dark));
    wiring::set_flyout_labels(&flyout, &English);
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

/// Renders the settings window with the demo's settings.
fn render_settings(world: &demo::Demo, scale: f64, out: &Path) -> Result<(), AppError> {
    let window = ui::SettingsWindow::new()?;
    let reported: Vec<(MeterKey, String)> = world
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
                    format!(
                        "{} {}",
                        s.snapshot.provider.display_name(),
                        m.names.short_name
                    ),
                )
            })
        })
        .collect();
    let draft = crate::settings_window::draft_with(world.settings.clone(), &reported);
    crate::settings_window::fill(&window, &draft, &reported, true);
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

/// Renders the about window.
fn render_about(scale: f64, out: &Path) -> Result<(), AppError> {
    let window = ui::AboutWindow::new()?;
    window.set_version(env!("CARGO_PKG_VERSION").into());
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
}

/// The flyout and the hover card from one synchronous read of every enabled service with the
/// real providers, the user's theme, regional format and history.
fn live_models() -> Result<LiveModels, AppError> {
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
    let mut state = AppState::new(settings, history, theme, locale);
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
            wiring::card_menu(card, &state.strings, &pins, usage)
        })
        .collect();
    Ok(LiveModels {
        flyout,
        menus,
        hover_card,
        theme,
    })
}
