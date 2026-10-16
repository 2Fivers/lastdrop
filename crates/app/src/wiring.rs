//! Start-up and the main thread's event handlers: the glue between the threads, the Slint
//! windows, the tray icon and `lastdrop-core`'s models (ARCHITECTURE "Threads", "Messages between
//! threads").
//!
//! Event sources, all handled on the main thread:
//! - refresh results from [`crate::scheduler`] (via `slint::invoke_from_event_loop`);
//! - readout events from `lastdrop_win::readout` (hover, leave, click, right click, placement,
//!   system broadcasts; via `slint::invoke_from_event_loop`);
//! - tray icon input from `lastdrop_win::tray` (our own `NOTIFYICON_VERSION_4` icon: select, key
//!   select, context menu, hover-card popup open/close, re-created after an Explorer restart); the
//!   context menu is shown with `Tray::show_menu` and returns the chosen command id;
//! - the optional global hotkey (`lastdrop_win::hotkey`, R§10);
//! - the second-instance listener (`lastdrop_win::single_instance`: open the flyout);
//! - the settings-changed listener (`lastdrop_win::single_instance`): the dialog process
//!   ([`crate::dialog`]) wrote `settings.json` or the Meshy key ([`on_settings_changed`]);
//! - Slint callbacks of the flyout and the hover card (settings and about run in the dialog
//!   process, which [`open_dialog`] starts);
//! - the minute tick: a `slint::Timer` at the next local minute boundary + 250 ms (R§12), re-armed
//!   after every tick, resume and time change;
//! - the refresh timer: one `slint::Timer` for the scheduler's next wake-up.
//!
//! On each new reading and each minute tick: evaluate (`lastdrop_core::stats::evaluate`), then
//! build the readout model (handed to the readout thread as an `Arc`), the tray model (icon image +
//! tooltip), and — only while visible — the hover card and the flyout (F§1.4: "Hidden: no timers,
//! no redraws").
//!
//! Re-entrancy: Win32 callbacks (window procedures of the tray, the hotkey, the listener and the
//! subclassed Slint windows) never touch [`App`] directly; they defer with
//! `slint::Timer::single_shot(Duration::ZERO, …)`. Handlers take short `RefCell` borrows of the
//! state and release them before any Win32 or Slint call that could run another handler.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use lastdrop_core::color::ColorRole;
use lastdrop_core::consts::layout::{
    CARD_BAR_ROW_H, CARD_BAR_W, CARD_EDGE_MARGIN, CARD_TASKBAR_GAP, CARD_W, FLYOUT_BAR_ROW_H,
    FLYOUT_BAR_W, FLYOUT_MAX_H, FLYOUT_W,
};
use lastdrop_core::consts::{ADAPTIVE_IDLE_AFTER, FLYOUT_TOGGLE_DEBOUNCE, MINUTE_TICK_OFFSET};
use lastdrop_core::flyout::{
    CardAction, CardBody, CardModel, FlyoutModel, MessageKind, MeterRowModel, RightKind,
    SparkModel, Tone, build_flyout, spark_geometry,
};
use lastdrop_core::glyph::{Icon, ProviderGlyph, SeverityGlyph, provider_paths};
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Msg, Strings};
use lastdrop_core::model::{MeterKey, ProviderId, SourceKey, catalog};
use lastdrop_core::notify::NotificationModel;
use lastdrop_core::readout::bar::{BarGeometry, bar_geometry};
use lastdrop_core::readout::hovercard::{AlsoLine, CardMeter, HoverCardModel};
use lastdrop_core::readout::{
    BuildContext, GlyphKind, HoverAnchor, ReadoutForm, ReadoutModel, TrayModel, build_hover_card,
    build_readout, build_tray,
};
use lastdrop_core::refresh::{Activity, manual_allowed};
use lastdrop_core::settings::{Pause, ReadoutContent, Settings};
use lastdrop_win::backdrop::{self, Backdrop, PopupStyle, WindowNotice};
use lastdrop_win::flyout_pos::{self, Anchor, MonitorInfo};
use lastdrop_win::geom::{Point, Rect, Size};
use lastdrop_win::hotkey::HotkeyRegistration;
use lastdrop_win::palette::{self, Argb};
use lastdrop_win::placement::{Placement, TaskbarEdge};
use lastdrop_win::readout::{ReadoutConfig, ReadoutEvent, ReadoutHandle, SystemEvent};
use lastdrop_win::render::RenderFactory;
use lastdrop_win::single_instance::{self, InstanceGuard, Listener};
use lastdrop_win::theme::ThemeInfo;
use lastdrop_win::tray::{MenuItem, NoticeIcon, Tray, TrayImage, TrayInput};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, ModelRc, SharedString, TimerMode, VecModel};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
};

use crate::dialog::{self, Changes, DialogKind};
use crate::history::HistoryStore;
use crate::persist::{LoadOutcome, Paths};
use crate::scheduler::{Job, JobResult, Scheduler, WorkerIo};
use crate::state::AppState;
use crate::writer::Writer;
use crate::{AppError, Args, bench, persist, ui};

/// What `main` hands over.
pub struct Startup {
    /// Command-line flags.
    pub args: Args,
    /// The single-instance mutex, held for the app's lifetime.
    pub guard: InstanceGuard,
    /// Folders.
    pub paths: Paths,
    /// Loaded settings.
    pub settings: Settings,
}

thread_local! {
    /// The running app, main thread only. Callbacks look it up instead of capturing an `Rc`, so
    /// shutdown can take it back.
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    /// The next Slint window created is a popup (flyout, hover card): the winit attributes hook
    /// reads it. Slint creates the window adapter synchronously in `Component::new()`.
    static NEXT_WINDOW_IS_POPUP: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` with the app, if it is running. The `Rc` is cloned out first, so `f` may itself call
/// `with_app` (no `RefCell` borrow is held while it runs).
pub fn with_app(f: impl FnOnce(&App)) {
    let app = APP.with(|a| a.borrow().clone());
    if let Some(app) = app {
        f(&app);
    }
}

/// Runs `f` with the app on the next turn of the event loop (from window procedures, which must
/// not re-enter Slint synchronously).
pub fn defer(f: impl FnOnce(&App) + 'static) {
    slint::Timer::single_shot(Duration::ZERO, move || with_app(f));
}

/// Creates a Slint window with the popup attributes (flyout, hover card) or the default ones
/// (settings and about, in the dialog process).
pub(crate) fn create_window<T>(
    popup: bool,
    new: impl FnOnce() -> Result<T, slint::PlatformError>,
) -> Result<T, AppError> {
    NEXT_WINDOW_IS_POPUP.with(|p| p.set(popup));
    let made = new();
    NEXT_WINDOW_IS_POPUP.with(|p| p.set(false));
    Ok(made?)
}

/// Selects the Slint backend of both processes: winit with the software renderer. Windows made
/// with [`create_window`]`(true, …)` get the spike's popup attributes (not active, no taskbar
/// entry, no decorations, always on top, off-screen, acrylic, round corners); the others winit's
/// defaults.
pub(crate) fn select_backend() -> Result<(), AppError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attrs| {
            use slint::winit_030::winit;
            use winit::platform::windows::{
                BackdropType, CornerPreference, WindowAttributesExtWindows,
            };
            if !NEXT_WINDOW_IS_POPUP.with(Cell::get) {
                return attrs;
            }
            attrs
                .with_active(false)
                .with_skip_taskbar(true)
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000))
                .with_system_backdrop(BackdropType::TransientWindow)
                .with_corner_preference(CornerPreference::Round)
        })
        .select()?;
    Ok(())
}

/// Runs the app until "Quit Lastdrop": DPI awareness (the manifest's per-monitor v2), dark menus,
/// theme and locale, history, the Slint backend (winit + software renderer, the spike's window
/// attributes: not active, no taskbar entry, no decorations, always on top, off-screen, acrylic,
/// round corners), the windows created hidden and kept alive, the tray icon, the readout thread,
/// the workers, the timers; then the event loop; then an orderly shutdown (tray icon removed,
/// readout and workers joined, history flushed).
pub fn run(startup: Startup) -> Result<(), AppError> {
    let Startup {
        args,
        guard,
        paths,
        settings,
    } = startup;
    let bench = args
        .bench
        .clone()
        .map(|out| Rc::new(bench::Bench::new(out)));
    if let Some(b) = &bench {
        b.phase("mainEntered");
    }

    // COM on the main thread (Direct2D/WIC for the tray icon, UISettings for the theme). winit
    // initializes OLE on the same apartment later; S_FALSE and a mode mismatch are fine.
    // SAFETY: called once on this thread before any COM object is created on it.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    backdrop::allow_dark_menus();

    let theme = lastdrop_win::theme::read();
    let locale = lastdrop_win::locale::read(TimeZone::system());
    let now = Timestamp::now();
    // Every file write from here on goes through the writer thread, never the UI thread.
    let writer = Writer::start()?;
    let history = HistoryStore::open(&paths.history, now, writer.handle())?;
    let readout_enabled = settings.readout.enabled && !args.no_readout;
    let hotkey_setting = settings.hotkey.clone();
    let mut state = AppState::new(settings, history, theme, locale);
    state.restore_saved(&persist::load_state(&paths.state), now);
    let state = Rc::new(RefCell::new(state));

    // The Slint backend: winit with the software renderer. Popups get the spike's attributes.
    select_backend()?;

    let flyout = create_window(true, ui::Flyout::new)?;
    let hover_card = create_window(true, ui::HoverCard::new)?;
    set_theme(&flyout.global::<ui::Theme>(), &theme);
    set_theme(&hover_card.global::<ui::Theme>(), &theme);
    set_flyout_labels(&flyout, &lastdrop_core::grammar::English);
    connect_flyout(&flyout);
    hover_card.on_clicked(|| with_app(hide_hover_card));

    // The readout thread (its events come back through the event loop).
    let readout = ReadoutHandle::spawn(
        ReadoutConfig {
            enabled: readout_enabled,
            hover_delay: None,
            theme,
        },
        Box::new(|event| {
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| on_readout_event(app, event));
            });
        }),
    )?;

    // The refresh workers.
    let scheduler = Scheduler::start(
        Arc::new(WorkerIo::new()),
        Arc::new(|result: JobResult| {
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| on_job_result(app, result));
            });
        }),
    )?;

    // The tray icon, drawn from the first evaluation (LOADING: the drop).
    let render = RenderFactory::new()?;
    let (tray_model, tray_dpi) = {
        let mut st = state.borrow_mut();
        st.evaluate(now);
        let st = &*st;
        let model = st.evaluation.as_ref().map(|eval| {
            let cx = BuildContext {
                text: st.text_context(now),
                settings: &st.settings,
            };
            build_tray(eval, &cx)
        });
        // SAFETY: GetDpiForSystem has no preconditions.
        (model, unsafe { GetDpiForSystem() })
    };
    let tray_model = tray_model.unwrap_or_else(|| TrayModel {
        icon: lastdrop_core::readout::TrayIcon::Drop,
        tooltip: "Lastdrop".to_owned(),
    });
    let image = tray_icon_image(&tray_model, tray_dpi, &theme, &render)?;
    let tray = Tray::create(
        &image,
        &tray_model.tooltip,
        Box::new(|input| defer(move |app| on_tray_input(app, input))),
    )?;
    if let Some(b) = &bench {
        b.phase("trayCreated");
    }

    let listener = guard.listen(Box::new(|| {
        defer(|app| {
            let anchor = tray_anchor(app);
            open_flyout(app, anchor, false);
        });
    }))?;
    // The dialog process says what it changed (settings.json, the Meshy key).
    let settings_listener = single_instance::listen_settings_changed(Box::new(|flags| {
        defer(move |app| on_settings_changed(app, Changes::from_wparam(flags)));
    }))?;

    let menu = {
        let st = state.borrow();
        TrayMenu::build(&st.strings, &MenuState::of(&st.settings, Timestamp::now()))
    };
    let app = Rc::new(App {
        state,
        windows: Windows { flyout, hover_card },
        readout,
        scheduler,
        tray,
        menu: RefCell::new(menu),
        render,
        listener,
        settings_listener,
        hotkey: RefCell::new(None),
        args: args.clone(),
        paths,
        writer,
        rt: Runtime {
            bench: bench.clone(),
            ..Runtime::default()
        },
    });
    *app.rt.last_tray.borrow_mut() = Some((tray_model, tray_dpi, theme));
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    if let Some(hotkey) = hotkey_setting {
        register_hotkey(&app, &hotkey);
    }

    // Keep-alive windows: shown off-screen and inactive so winit creates them, then styled and
    // hidden with Win32 (F§1.1, R§8.1).
    app.windows.flyout.show()?;
    app.windows.hover_card.show()?;
    let open_on_start = !args.autostart && args.smoke.is_none() && args.bench.is_none();
    setup_popup(app.windows.flyout.as_weak(), false, open_on_start);
    setup_popup(app.windows.hover_card.as_weak(), true, false);

    // First evaluation, the readout model, the first refresh round and the timers.
    rebuild(&app, Timestamp::now());
    replan(&app);
    arm_minute_tick(&app);
    if let Some(after) = args.smoke {
        slint::Timer::single_shot(after, || {
            log::info!("smoke run finished");
            let _ = slint::quit_event_loop();
        });
    }
    if let Some(b) = &bench {
        bench::schedule(b.clone());
    }
    drop(app);

    slint::run_event_loop_until_quit()?;

    shutdown();
    Ok(())
}

/// The orderly shutdown after the event loop: windows hidden, hotkey unregistered, tray icon
/// removed (`NIM_DELETE` on drop), the second-instance and settings-changed listeners closed,
/// readout joined, queued refreshes dropped and running ones given `scheduler::SHUTDOWN_WAIT`.
fn shutdown() {
    let Some(app) = APP.with(|a| a.borrow_mut().take()) else {
        return;
    };
    if let Some(b) = &app.rt.bench {
        b.phase("exit");
        b.write(&app, false);
    }
    app.rt.minute.stop();
    app.rt.refresh.stop();
    if let Some(hotkey) = app.hotkey.borrow_mut().take() {
        hotkey.unregister();
    }
    match Rc::try_unwrap(app) {
        Ok(app) => {
            let App {
                tray,
                readout,
                scheduler,
                windows,
                listener,
                settings_listener,
                writer,
                ..
            } = app;
            // The tray icon and the readout go first: a slow disk below must not leave them
            // on screen for a process that is ending.
            drop(tray);
            // A second launch from now on finds no listener and exits; it must not signal an
            // instance whose event loop has ended. Nor may the dialog process.
            drop(listener);
            drop(settings_listener);
            readout.shutdown();
            // Every queued write (history, settings, state) reaches the disk, within
            // `writer::SHUTDOWN_WAIT`, before the threads end.
            writer.shutdown();
            scheduler.shutdown();
            drop(windows);
        }
        Err(app) => {
            app.writer.shutdown();
            log::warn!("shutdown: the app is still referenced; threads end with the process");
        }
    }
    log::info!("stopped");
}

/// Styles a keep-alive popup once winit has created its window: DWM backdrop and corners, tool
/// window, then hidden (`backdrop::apply`); the flyout also gets the deactivation watch.
fn setup_popup<T: ComponentHandle + 'static>(weak: slint::Weak<T>, hover: bool, open_after: bool) {
    let spawned = slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if let Err(e) = window.window().winit_window().await {
            log::error!("window not created: {e}");
            return;
        }
        let Some(hwnd) = hwnd_of_window(window.window()) else {
            log::error!("no Win32 window handle");
            return;
        };
        with_app(|app| {
            let theme = app.state.borrow().theme;
            let style = PopupStyle {
                backdrop: if theme.high_contrast.is_some() {
                    Backdrop::None
                } else {
                    Backdrop::Acrylic
                },
                round_corners: true,
                dark: theme.taskbar_dark,
                no_activate: hover,
            };
            if let Err(e) = backdrop::apply(hwnd, &style) {
                log::warn!("popup style: {e}");
            }
            let raw = hwnd.0 as isize;
            if hover {
                app.rt.hover_hwnd.set(raw);
                return;
            }
            app.rt.flyout_hwnd.set(raw);
            if let Err(e) = backdrop::watch(
                hwnd,
                Box::new(|notice| defer(move |app| on_flyout_notice(app, notice))),
            ) {
                log::warn!("flyout watch: {e}");
            }
            if let Some(b) = &app.rt.bench {
                bench::watch_paint(hwnd, b.clone());
                b.phase("windowPrecreatedHidden");
            }
            if open_after {
                let anchor = tray_anchor(app);
                open_flyout(app, anchor, false);
            }
        });
    });
    if let Err(e) = spawned {
        log::error!("cannot set up a window: {e}");
    }
}

/// The Win32 handle of a Slint window (winit backend).
pub(crate) fn hwnd_of_window(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut core::ffi::c_void)),
        _ => None,
    }
}

/// A stored window handle (0 = not created yet).
fn hwnd(raw: isize) -> Option<HWND> {
    (raw != 0).then_some(HWND(raw as *mut core::ffi::c_void))
}

/// The tray process's Slint windows, created hidden at start-up and kept alive (F§1.1: "open in a
/// few ms"). Settings and about are windows of the dialog process ([`crate::dialog`]).
pub struct Windows {
    /// The flyout.
    pub flyout: ui::Flyout,
    /// The hover card.
    pub hover_card: ui::HoverCard,
}

/// A tray menu command (R§10 context menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuCommand {
    /// Refresh now.
    Refresh,
    /// Pause updates ▸ For 1 hour.
    PauseHour,
    /// Pause updates ▸ Until I resume.
    PauseUntilResume,
    /// Resume updates (shown instead of Pause while paused).
    Resume,
    /// Taskbar readout ▸ Auto.
    ReadoutAuto,
    /// Taskbar readout ▸ Pinned meters… (opens settings at the pins, in the dialog process).
    ReadoutPinned,
    /// Taskbar readout ▸ Off.
    ReadoutOff,
    /// Settings… (the dialog process).
    Settings,
    /// About Lastdrop (required by Slint's licence; the dialog process).
    About,
    /// Quit Lastdrop.
    Quit,
}

/// What the tray menu reflects: pause state and the readout mode (check marks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    /// Updates are paused now: the menu offers "Resume updates" instead of "Pause updates ▸".
    pub paused: bool,
    /// The readout is on.
    pub readout_on: bool,
    /// The readout shows pinned meters (else auto).
    pub readout_pinned: bool,
}

impl MenuState {
    /// The menu state of `settings` at `now`.
    pub fn of(settings: &Settings, now: Timestamp) -> MenuState {
        MenuState {
            paused: pause_active(settings, now),
            readout_on: settings.readout.enabled,
            readout_pinned: matches!(settings.readout.content, ReadoutContent::Pinned(_)),
        }
    }
}

/// Whether `settings.pause` is in effect at `now`.
pub fn pause_active(settings: &Settings, now: Timestamp) -> bool {
    settings
        .pause
        .is_some_and(|p| p.until.is_none_or(|until| now < until))
}

/// The tray context menu (Win32 popup menu through `lastdrop_win::tray::Tray::show_menu`, dark-mode
/// aware through `lastdrop_win::backdrop::allow_dark_menus`).
pub struct TrayMenu {
    /// The items handed to `Tray::show_menu`.
    pub items: Vec<MenuItem>,
    /// Item ids → commands.
    pub ids: Vec<(u32, MenuCommand)>,
}

impl TrayMenu {
    /// Builds the R§10 menu with localized labels (`Msg::Menu*`): **Refresh now** · **Pause
    /// updates** ▸ (for 1 hour / until I resume), or **Resume updates** while paused ·
    /// **Taskbar readout** ▸ (Auto / Pinned meters… / Off, the current one checked) ·
    /// **Settings…** · **About Lastdrop** · **Quit Lastdrop**.
    pub fn build(strings: &dyn Strings, state: &MenuState) -> TrayMenu {
        let mut ids = Vec::new();
        let mut command = |command: MenuCommand, msg: Msg, checked: bool| {
            let id = u32::try_from(ids.len() + 1).unwrap_or(u32::MAX);
            ids.push((id, command));
            MenuItem::Command {
                id,
                label: strings.template(msg).to_owned(),
                checked,
                enabled: true,
                default: false,
            }
        };
        let mut items = vec![command(MenuCommand::Refresh, Msg::MenuRefresh, false)];
        if state.paused {
            items.push(command(MenuCommand::Resume, Msg::MenuResume, false));
        } else {
            let hour = command(MenuCommand::PauseHour, Msg::MenuPauseHour, false);
            let until = command(
                MenuCommand::PauseUntilResume,
                Msg::MenuPauseUntilResume,
                false,
            );
            items.push(MenuItem::Submenu {
                label: strings.template(Msg::MenuPause).to_owned(),
                items: vec![hour, until],
            });
        }
        let auto = command(
            MenuCommand::ReadoutAuto,
            Msg::MenuReadoutAuto,
            state.readout_on && !state.readout_pinned,
        );
        let pinned = command(
            MenuCommand::ReadoutPinned,
            Msg::MenuReadoutPinned,
            state.readout_on && state.readout_pinned,
        );
        let off = command(
            MenuCommand::ReadoutOff,
            Msg::MenuReadoutOff,
            !state.readout_on,
        );
        items.push(MenuItem::Submenu {
            label: strings.template(Msg::MenuReadout).to_owned(),
            items: vec![auto, pinned, off],
        });
        items.push(MenuItem::Separator);
        items.push(command(MenuCommand::Settings, Msg::MenuSettings, false));
        items.push(command(MenuCommand::About, Msg::MenuAbout, false));
        items.push(MenuItem::Separator);
        items.push(command(MenuCommand::Quit, Msg::MenuQuit, false));
        TrayMenu { items, ids }
    }

    /// The command of a menu event's id.
    pub fn command(&self, id: u32) -> Option<MenuCommand> {
        self.ids.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
    }
}

/// What one flyout card's actions refer to (the card index → source, message action, pins).
#[derive(Clone, Debug, PartialEq)]
pub struct CardRef {
    /// The card's source.
    pub source: SourceKey,
    /// The message line's link action.
    pub action: Option<CardAction>,
    /// The meters of the card's "Show in taskbar readout ▸" checklist, in order.
    pub pins: Vec<MeterKey>,
}

/// The per-card menu data (F§9 card menu) that the flyout model does not carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardMenu {
    /// `Refresh Claude`.
    pub refresh_label: String,
    /// The service has a usage page.
    pub has_usage_page: bool,
    /// `(meter name, pinned)` for every meter of the card.
    pub pins: Vec<(String, bool)>,
}

/// Main-thread runtime bookkeeping besides [`AppState`].
#[derive(Default)]
pub struct Runtime {
    /// The flyout's `HWND` (0 until winit created it).
    pub flyout_hwnd: Cell<isize>,
    /// The hover card's `HWND`.
    pub hover_hwnd: Cell<isize>,
    /// Where the open flyout is (device px), for bottom-anchored resizes.
    pub flyout_rect: Cell<Option<Rect>>,
    /// The monitor the open flyout is on.
    pub flyout_monitor: Cell<Option<MonitorInfo>>,
    /// The visible hover card: what it shows and the rect it is centred on.
    pub hover: Cell<Option<(HoverAnchor, Rect)>>,
    /// The readout's slot on screen (flyout anchor for readout clicks).
    pub readout_rect: Cell<Option<Rect>>,
    /// The readout's slot while it floats over the work area (no room on the taskbar): the
    /// flyout keeps clear of it (`flyout_pos::flyout_max_height`).
    pub readout_floating: Cell<Option<Rect>>,
    /// What the open flyout was placed at, for resizes.
    pub flyout_anchor: Cell<Option<Anchor>>,
    /// The last readout model (its compact row, R§8.2 rows).
    pub last_readout: RefCell<Option<Arc<ReadoutModel>>>,
    /// The tray model, DPI and theme the current icon was drawn for.
    pub last_tray: RefCell<Option<(TrayModel, u32, ThemeInfo)>>,
    /// The cards the flyout shows.
    pub cards: RefCell<Vec<CardRef>>,
    /// The card to focus when the flyout opens by a click (sign-in, R§10).
    pub focus_card: Cell<Option<usize>>,
    /// Sources of a running manual refresh (the refresh button shows progress, F§3).
    pub manual: RefCell<BTreeSet<SourceKey>>,
    /// The minute tick (R§12).
    pub minute: slint::Timer,
    /// The scheduler's next wake-up.
    pub refresh: slint::Timer,
    /// While a menu of our own has the foreground, polls whether the user left the flyout.
    pub foreground: slint::Timer,
    /// Clears the flyout's live-region announcement.
    pub announce: slint::Timer,
    /// xorshift state for the reset jitter (R§3.9).
    pub rng: Cell<u64>,
    /// The local date history files were last compacted.
    pub last_compact: Cell<Option<jiff::civil::Date>>,
    /// Whether the user was idle when the refresh was last planned ([`user_idle`]); the minute
    /// tick replans when that changes.
    pub planned_idle: Cell<bool>,
    /// `--bench`.
    pub bench: Option<Rc<bench::Bench>>,
}

/// Everything the handlers need, shared by the closures of the event loop (main thread only).
pub struct App {
    /// The state.
    pub state: Rc<RefCell<AppState>>,
    /// The windows.
    pub windows: Windows,
    /// The readout thread.
    pub readout: ReadoutHandle,
    /// The refresh workers.
    pub scheduler: Scheduler,
    /// The tray icon (dropping it removes it: `NIM_DELETE`); input arrives as [`TrayInput`].
    pub tray: Tray,
    /// The tray menu ids of the menu shown last.
    pub menu: RefCell<TrayMenu>,
    /// Direct2D/DirectWrite/WIC of the main thread (tray icon images).
    pub render: RenderFactory,
    /// The second-instance listener.
    pub listener: Listener,
    /// The settings-changed listener (the dialog process wrote settings or the Meshy key).
    pub settings_listener: Listener,
    /// The global hotkey, when set in settings (R§10).
    pub hotkey: RefCell<Option<HotkeyRegistration>>,
    /// Command-line flags.
    pub args: Args,
    /// Folders.
    pub paths: Paths,
    /// The file writer thread: history, settings and state writes, in queue order.
    pub writer: Writer,
    /// Runtime bookkeeping.
    pub rt: Runtime,
}

impl App {
    /// The readout is allowed in this run (`--no-readout` forbids it).
    pub fn readout_allowed(&self, settings: &Settings) -> bool {
        settings.readout.enabled && !self.args.no_readout
    }

    /// A uniform value in `[0, 1)` for the reset jitter.
    fn jitter_unit(&self) -> f64 {
        let mut x = self.rt.rng.get();
        if x == 0 {
            x = Timestamp::now().as_nanosecond().unsigned_abs() as u64 | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rt.rng.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---- Refreshes ------------------------------------------------------------------------------------

/// A refresh result arrived: apply it, record history, rebuild the models, re-plan the timer.
pub fn on_job_result(app: &App, result: JobResult) {
    let source = result.source.clone();
    app.rt.manual.borrow_mut().remove(&source);
    {
        let mut st = app.state.borrow_mut();
        st.apply_result(result);
        st.refreshing = !app.rt.manual.borrow().is_empty();
    }
    rebuild(app, Timestamp::now());
    replan(app);
}

/// The user is idle for the adaptive refresh ("slower when idle"): no keyboard or mouse input
/// for [`ADAPTIVE_IDLE_AFTER`]. Unknown counts as not idle.
fn user_idle() -> bool {
    lastdrop_win::idle::since_last_input().is_some_and(|d| d >= ADAPTIVE_IDLE_AFTER)
}

/// Asks the scheduler what is due, submits the jobs, records `next_try` per source and arms the
/// refresh timer for the next wake-up.
pub fn replan(app: &App) {
    let now = Timestamp::now();
    let idle = user_idle();
    app.rt.planned_idle.set(idle);
    let plan = {
        let st = app.state.borrow();
        let activity = Activity {
            flyout_open: st.flyout.open,
            user_idle: idle,
        };
        crate::scheduler::plan(&st, now, activity, &mut || app.jitter_unit())
    };
    let mut jobs = Vec::new();
    {
        let mut st = app.state.borrow_mut();
        for (key, source) in st.sources.iter_mut() {
            source.status.next_try = plan.next_try.get(key).copied();
        }
        for key in &plan.due {
            if let Some(source) = st.sources.get_mut(key) {
                source.refresh.in_flight = true;
                jobs.push(Job {
                    source: key.clone(),
                    provider: source.provider.clone(),
                    account: source.source.clone(),
                    manual: false,
                    probe: None,
                });
            }
        }
        for key in &plan.probes {
            if let Some(source) = st.sources.get_mut(key) {
                source.refresh.in_flight = true;
                jobs.push(Job {
                    source: key.clone(),
                    provider: source.provider.clone(),
                    account: source.source.clone(),
                    manual: false,
                    probe: source.refresh.auth_stopped.clone(),
                });
            }
        }
    }
    for job in jobs {
        app.scheduler.submit(job);
    }
    match plan.next_wake {
        Some(at) => {
            let delay = Duration::try_from(at.duration_since(Timestamp::now()))
                .unwrap_or(Duration::ZERO)
                .max(Duration::from_millis(250));
            app.rt
                .refresh
                .start(TimerMode::SingleShot, delay, || with_app(replan));
        }
        None => app.rt.refresh.stop(),
    }
}

/// "Refresh now", F5, the refresh button, `Retry` and the card menu (F§3): every source (or only
/// `only`) whose provider allows it now (`refresh::manual_allowed`: minimum interval and back-off;
/// a source that cannot be refreshed yet keeps its timer).
pub fn refresh_now(app: &App, only: Option<&SourceKey>) {
    let now = Timestamp::now();
    let mut jobs = Vec::new();
    {
        let mut st = app.state.borrow_mut();
        for (key, source) in st.sources.iter_mut() {
            if only.is_some_and(|o| o != key) {
                continue;
            }
            if !manual_allowed(&source.refresh, now, source.provider.min_interval()) {
                continue;
            }
            source.refresh.in_flight = true;
            jobs.push(Job {
                source: key.clone(),
                provider: source.provider.clone(),
                account: source.source.clone(),
                manual: true,
                probe: None,
            });
        }
        if !jobs.is_empty() {
            st.refreshing = true;
        }
    }
    if jobs.is_empty() {
        return;
    }
    for job in jobs {
        app.rt.manual.borrow_mut().insert(job.source.clone());
        app.scheduler.submit(job);
    }
    rebuild(app, now);
}

// ---- Minute tick ------------------------------------------------------------------------------------

/// Arms the minute tick for the next local minute boundary + 250 ms (R§12).
fn arm_minute_tick(app: &App) {
    let now = Timestamp::now();
    let tz = app.state.borrow().locale.tz.clone();
    let at = next_minute_tick(now, &tz);
    let delay = Duration::try_from(at.duration_since(now)).unwrap_or(Duration::from_secs(60));
    app.rt
        .minute
        .start(TimerMode::SingleShot, delay, || with_app(on_minute_tick));
}

/// The minute tick (R§12): recompute `N` and severities (a WARNING can turn CRITICAL by runway
/// between readings), then freshness, then redraw; trim history memory; re-arm. Also replans
/// when the user went idle or came back since the last plan, so the adaptive interval follows
/// within a minute without a wake-up of its own (an idle wait can be up to 30 minutes).
pub fn on_minute_tick(app: &App) {
    let now = Timestamp::now();
    let mut pause_ended = false;
    {
        let mut st = app.state.borrow_mut();
        if st.settings.pause.is_some() && !pause_active(&st.settings, now) {
            st.settings.pause = None;
            pause_ended = true;
        }
        st.history.trim_memory(now);
        let today = now.to_zoned(st.locale.tz.clone()).date();
        if app.rt.last_compact.get() != Some(today) {
            app.rt.last_compact.set(Some(today));
            // Queued behind the appends so far; the writer thread runs it.
            st.history.compact(now);
        }
    }
    if pause_ended {
        save_settings(app);
    }
    if pause_ended || user_idle() != app.rt.planned_idle.get() {
        replan(app);
    }
    rebuild(app, now);
    arm_minute_tick(app);
}

/// The next minute tick: the next local minute boundary after `now` in `tz`, plus
/// `MINUTE_TICK_OFFSET` (250 ms) (R§12).
pub fn next_minute_tick(now: Timestamp, tz: &TimeZone) -> Timestamp {
    let zoned = now.to_zoned(tz.clone());
    let into_minute = SignedDuration::new(i64::from(zoned.second()), zoned.subsec_nanosecond());
    let offset = SignedDuration::try_from(MINUTE_TICK_OFFSET).unwrap_or(SignedDuration::ZERO);
    now.checked_sub(into_minute)
        .and_then(|t| t.checked_add(SignedDuration::from_secs(60)))
        .and_then(|t| t.checked_add(offset))
        .unwrap_or(now)
}

// ---- Readout and tray input ---------------------------------------------------------------------

/// A readout event (R§10): hover → hover card after the delay; leave → hide (100 ms grace);
/// click → toggle the flyout (sign-in card focused when a service needs sign-in); UIA invoke →
/// open as a keyboard open; right click → the context menu at the point; placement → remember
/// form/rows/monitor/edge, re-render the tray icon if the DPI changed, rebuild; system → theme
/// (read once, then `ReadoutHandle::set_theme`, tray icon, Slint `Theme` globals), locale,
/// re-arm the minute tick.
pub fn on_readout_event(app: &App, event: ReadoutEvent) {
    match event {
        ReadoutEvent::Hover { rect } => show_hover_card(app, HoverAnchor::Readout, rect),
        ReadoutEvent::Leave => hide_hover_card(app),
        ReadoutEvent::Click => {
            hide_hover_card(app);
            let anchor = readout_anchor(app);
            toggle_flyout(app, anchor, false);
        }
        ReadoutEvent::Invoked => {
            hide_hover_card(app);
            let anchor = readout_anchor(app);
            open_flyout(app, anchor, true);
        }
        ReadoutEvent::RightClick { pt } => {
            hide_hover_card(app);
            show_tray_menu(app, pt);
        }
        ReadoutEvent::PlacementChanged {
            placement,
            form,
            monitor,
            taskbar,
            edge,
        } => {
            app.rt.readout_rect.set(match placement {
                Placement::Docked { rect, .. } | Placement::Floating { rect, .. } => Some(rect),
                Placement::Hidden(_) => None,
            });
            let floating = match placement {
                Placement::Floating { rect, .. } => Some(rect),
                Placement::Docked { .. } | Placement::Hidden(_) => None,
            };
            let floating_changed = app.rt.readout_floating.replace(floating) != floating;
            {
                let mut st = app.state.borrow_mut();
                let rows = shown_rows(&st, app.rt.last_readout.borrow().as_deref(), form);
                st.readout = crate::state::ReadoutState {
                    form,
                    rows,
                    monitor: Some(monitor),
                    taskbar,
                    edge,
                };
            }
            // A readout that starts (or stops) floating while the flyout is open moves the edge
            // the flyout keeps clear of it (F§1.2, F§1.3).
            if floating_changed {
                replace_open_flyout(app);
            }
            update_tray(app, false);
        }
        ReadoutEvent::System(system) => on_system_event(app, system),
    }
}

/// The rows the readout draws in `form` (the compact form shows one, hidden none), for the
/// hover card (R§8.2).
fn shown_rows(st: &AppState, model: Option<&ReadoutModel>, form: ReadoutForm) -> Vec<MeterKey> {
    match (form, model) {
        (ReadoutForm::Hidden, _) => Vec::new(),
        (ReadoutForm::Compact, Some(m)) => m
            .rows
            .get(m.compact_row)
            .map(|r| vec![r.key.clone()])
            .unwrap_or_default(),
        (_, Some(m)) => m.rows.iter().map(|r| r.key.clone()).collect(),
        (_, None) => st
            .evaluation
            .as_ref()
            .map(|e| e.readout_rows.clone())
            .unwrap_or_default(),
    }
}

/// System broadcasts seen by the readout thread.
fn on_system_event(app: &App, event: SystemEvent) {
    match event {
        SystemEvent::Theme(_) => apply_theme(app),
        SystemEvent::Locale => {
            let tz = TimeZone::system();
            app.state.borrow_mut().locale = lastdrop_win::locale::read(tz);
            rebuild(app, Timestamp::now());
        }
        SystemEvent::TimeChanged => {
            let tz = TimeZone::system();
            app.state.borrow_mut().locale.tz = tz;
            rebuild(app, Timestamp::now());
            arm_minute_tick(app);
        }
        SystemEvent::Resumed => {
            rebuild(app, Timestamp::now());
            arm_minute_tick(app);
            replan(app);
        }
    }
}

/// Re-reads the theme once and hands the same value to the readout, the tray icon, the popups
/// and their DWM attributes, so they cannot disagree.
///
/// One switch of light/dark, accent or contrast arrives as several messages (`WM_SETTINGCHANGE`
/// "ImmersiveColorSet", `WM_DWMCOLORIZATIONCOLORCHANGED`, `WM_THEMECHANGED`, `WM_SYSCOLORCHANGE`,
/// to the readout's host window and to each popup): only the first that finds a different theme
/// applies it, the rest change nothing and return here.
fn apply_theme(app: &App) {
    let theme = lastdrop_win::theme::read();
    if app.state.borrow().theme == theme {
        return;
    }
    app.state.borrow_mut().theme = theme;
    app.readout.set_theme(theme);
    set_theme(&app.windows.flyout.global::<ui::Theme>(), &theme);
    set_theme(&app.windows.hover_card.global::<ui::Theme>(), &theme);
    let backdrop_kind = if theme.high_contrast.is_some() {
        Backdrop::None
    } else {
        Backdrop::Acrylic
    };
    for raw in [app.rt.flyout_hwnd.get(), app.rt.hover_hwnd.get()] {
        if let Some(h) = hwnd(raw)
            && let Err(e) = backdrop::set_theme(h, theme.taskbar_dark, backdrop_kind)
        {
            log::warn!("popup theme: {e}");
        }
    }
    update_tray(app, true);
    rebuild(app, Timestamp::now());
}

/// Tray icon input (R§10): `PopupOpen`/`PopupClose` → hover card (`HoverAnchor::TrayIcon`);
/// `Select` → toggle the flyout (300 ms debounce after a deactivation hide, F§1.4); `KeySelect` →
/// open with focus on the first card; `ContextMenu` → `Tray::show_menu` at the point, then
/// [`on_menu_command`]; `Recreated` → re-render and re-apply the icon (the DPI may have changed);
/// `NotificationClicked` → open the flyout (N§4).
pub fn on_tray_input(app: &App, input: TrayInput) {
    match input {
        TrayInput::PopupOpen { pt } => {
            let rect = app.tray.icon_rect().unwrap_or(Rect {
                left: pt.x,
                top: pt.y,
                right: pt.x + 1,
                bottom: pt.y + 1,
            });
            let readout_visible = app.state.borrow().readout.form != ReadoutForm::Hidden;
            show_hover_card(app, HoverAnchor::TrayIcon { readout_visible }, rect);
        }
        TrayInput::PopupClose => hide_hover_card(app),
        TrayInput::Select => {
            hide_hover_card(app);
            let anchor = tray_anchor(app);
            toggle_flyout(app, anchor, false);
        }
        TrayInput::KeySelect => {
            hide_hover_card(app);
            let anchor = tray_anchor(app);
            open_flyout(app, anchor, true);
        }
        TrayInput::ContextMenu { pt } => {
            hide_hover_card(app);
            show_tray_menu(app, pt);
        }
        TrayInput::Recreated => update_tray(app, true),
        TrayInput::NotificationClicked => {
            hide_hover_card(app);
            let anchor = tray_anchor(app);
            open_flyout(app, anchor, false);
        }
    }
}

/// Shows the R§10 context menu at `pt` and runs the chosen command.
fn show_tray_menu(app: &App, pt: Point) {
    let menu = {
        let st = app.state.borrow();
        TrayMenu::build(&st.strings, &MenuState::of(&st.settings, Timestamp::now()))
    };
    let items = menu.items.clone();
    *app.menu.borrow_mut() = menu;
    let chosen = app.tray.show_menu(pt, &items);
    let command = chosen.and_then(|id| app.menu.borrow().command(id));
    if let Some(command) = command {
        on_menu_command(app, command);
    }
}

/// A tray menu command.
pub fn on_menu_command(app: &App, command: MenuCommand) {
    let now = Timestamp::now();
    match command {
        MenuCommand::Refresh => refresh_now(app, None),
        MenuCommand::PauseHour | MenuCommand::PauseUntilResume => {
            let until = (command == MenuCommand::PauseHour)
                .then(|| now.checked_add(SignedDuration::from_hours(1)).ok())
                .flatten();
            app.state.borrow_mut().settings.pause = Some(Pause { until });
            save_settings(app);
            rebuild(app, now);
            replan(app);
        }
        MenuCommand::Resume => resume(app),
        MenuCommand::ReadoutAuto | MenuCommand::ReadoutOff => {
            let enabled = command == MenuCommand::ReadoutAuto;
            let allowed = {
                let mut st = app.state.borrow_mut();
                st.settings.readout.enabled = enabled;
                if enabled {
                    st.settings.readout.content = ReadoutContent::Auto;
                }
                app.readout_allowed(&st.settings)
            };
            app.readout.set_enabled(allowed);
            save_settings(app);
            rebuild(app, now);
        }
        MenuCommand::ReadoutPinned => open_dialog(app, DialogKind::SettingsPins),
        MenuCommand::Settings => open_dialog(app, DialogKind::Settings),
        MenuCommand::About => open_dialog(app, DialogKind::About),
        MenuCommand::Quit => {
            hide_flyout(app, false);
            hide_hover_card(app);
            let _ = slint::quit_event_loop();
        }
    }
}

/// Ends a pause (menu "Resume updates", the message line's and the footer's `Resume`).
fn resume(app: &App) {
    app.state.borrow_mut().settings.pause = None;
    save_settings(app);
    rebuild(app, Timestamp::now());
    replan(app);
}

/// Saves the current settings: encoded here, written atomically by the writer thread in queue
/// order (a later save replaces an earlier one; failures are logged there).
pub fn save_settings(app: &App) {
    let bytes = persist::settings_bytes(&app.state.borrow().settings);
    match bytes {
        Ok(bytes) => app
            .writer
            .handle()
            .replace(app.paths.settings.clone(), bytes, "settings"),
        Err(e) => log::error!("cannot encode settings: {e}"),
    }
}

/// Registers the global hotkey (R§10); a failure is logged (the dialog process tested the
/// combination before saving it, so this fails only when another program took it since).
pub fn register_hotkey(app: &App, hotkey: &lastdrop_core::settings::Hotkey) -> Option<String> {
    if let Some(old) = app.hotkey.borrow_mut().take() {
        old.unregister();
    }
    let registered = lastdrop_win::hotkey::register(
        hotkey,
        Box::new(|| {
            defer(|app| {
                let anchor = tray_anchor(app);
                toggle_flyout(app, anchor, true);
            });
        }),
    );
    match registered {
        Ok(registration) => {
            *app.hotkey.borrow_mut() = Some(registration);
            None
        }
        Err(e) => {
            log::warn!("hotkey: {e}");
            Some(e.to_string())
        }
    }
}

/// Settings and About (R§10): starts the dialog process ([`dialog::spawn`]; a dialog process
/// that runs already opens the dialog or brings it to the front), then hides the flyout. In that
/// order: while the flyout (or the tray menu's window) is still in front this process holds the
/// foreground right it passes on to the dialog.
pub fn open_dialog(app: &App, kind: DialogKind) {
    if let Err(e) = dialog::spawn(kind) {
        log::error!("cannot start the dialog process ({}): {e}", kind.flag());
    }
    hide_flyout(app, false);
}

/// The dialog process changed something (already deferred to the event loop):
/// - `settings`: re-read `settings.json` ([`reloaded_settings`]; a missing or broken file is
///   ignored) and apply it as Apply did in this process: hotkey re-registered when it changed,
///   providers and sources rebuilt, readout on or off, models rebuilt, refreshes re-planned. The
///   file is not saved again here (it is the dialog's write, and saving would race it).
/// - `meshy_key`: lift Meshy's sign-in stop ([`crate::state::lift_sign_in_stop`]) and re-plan.
pub fn on_settings_changed(app: &App, changes: Changes) {
    log::info!("the dialog process changed {changes:?}");
    if changes.settings {
        let current = app.state.borrow().settings.clone();
        match reloaded_settings(&current, persist::read_settings(&app.paths.settings)) {
            Some(settings) => apply_reloaded(app, &current, settings),
            None => {
                log::warn!("settings.json is missing or unreadable; keeping the current settings");
            }
        }
    }
    if changes.meshy_key {
        crate::state::lift_sign_in_stop(&mut app.state.borrow_mut().sources, ProviderId::Meshy);
    }
    rebuild(app, Timestamp::now());
    replan(app);
}

/// The settings to apply after the dialog process wrote `settings.json`: the file's, with this
/// process's pause (the tray menu owns it, and a pause queued for the writer thread may not be in
/// the file yet). `None` when the file is missing or cannot be read or understood: a broken file
/// never replaces the running settings with defaults (the next start sets it aside).
pub fn reloaded_settings(
    current: &Settings,
    read: std::io::Result<(Settings, LoadOutcome)>,
) -> Option<Settings> {
    match read {
        Ok((settings, LoadOutcome::Loaded | LoadOutcome::Migrated { .. })) => Some(Settings {
            pause: current.pause,
            ..settings
        }),
        Ok((_, LoadOutcome::Missing | LoadOutcome::Corrupt { .. })) | Err(_) => None,
    }
}

/// Applies settings the dialog process saved: hotkey (when it changed), state, readout.
fn apply_reloaded(app: &App, previous: &Settings, settings: Settings) {
    if settings.hotkey != previous.hotkey {
        match &settings.hotkey {
            Some(hotkey) => {
                // A failure is logged there.
                let _ = register_hotkey(app, hotkey);
            }
            None => {
                if let Some(old) = app.hotkey.borrow_mut().take() {
                    old.unregister();
                }
            }
        }
    }
    let allowed = app.readout_allowed(&settings);
    app.state.borrow_mut().apply_settings(settings);
    app.readout.set_enabled(allowed);
}

// ---- Flyout ------------------------------------------------------------------------------------------

/// The tray icon's anchor (`Shell_NotifyIconGetRect`), else the cursor position.
fn tray_anchor(app: &App) -> Anchor {
    if let Some(rect) = app
        .tray
        .icon_rect()
        .filter(|r| r.width() > 0 && r.height() > 0)
    {
        return Anchor::Rect(rect);
    }
    let mut pt = POINT::default();
    // SAFETY: `pt` is a valid out pointer for the duration of the call.
    if unsafe { GetCursorPos(&mut pt) }.is_ok() {
        Anchor::Point(Point { x: pt.x, y: pt.y })
    } else {
        Anchor::Point(Point::default())
    }
}

/// The readout's slot, else the tray icon.
fn readout_anchor(app: &App) -> Anchor {
    match app.rt.readout_rect.get() {
        Some(rect) => Anchor::Rect(rect),
        None => tray_anchor(app),
    }
}

/// Toggles the flyout from `anchor` (F§1.3–F§1.4): position on the anchor's monitor, size from
/// `content-height`, `show_and_activate`, repaint tick; or hide.
pub fn toggle_flyout(app: &App, anchor: Anchor, by_keyboard: bool) {
    let (open, last_auto_hide) = {
        let st = app.state.borrow();
        (st.flyout.open, st.flyout.last_auto_hide)
    };
    if open {
        hide_flyout(app, false);
        return;
    }
    // The click that deactivated (and so hid) the flyout was meant to close it (F§1.4).
    if !by_keyboard && last_auto_hide.is_some_and(|t| t.elapsed() < FLYOUT_TOGGLE_DEBOUNCE) {
        app.state.borrow_mut().flyout.last_auto_hide = None;
        return;
    }
    open_flyout(app, anchor, by_keyboard);
}

/// Opens (or re-focuses) the flyout at `anchor`.
pub fn open_flyout(app: &App, anchor: Anchor, by_keyboard: bool) {
    let Some(h) = hwnd(app.rt.flyout_hwnd.get()) else {
        // Not created yet (the first frames after start-up).
        return;
    };
    hide_hover_card(app);
    let was_open = {
        let mut st = app.state.borrow_mut();
        let was = st.flyout.open;
        st.flyout.open = true;
        st.flyout.opened_by_keyboard = by_keyboard;
        was
    };
    let monitor = flyout_pos::monitor_for(anchor);
    app.rt.flyout_monitor.set(Some(monitor));
    // One evaluation for every surface; the flyout content is set because it is open now.
    rebuild(app, Timestamp::now());
    let focus_card = app.rt.focus_card.get();

    if !was_open {
        let edge = app.state.borrow().readout.edge;
        place_flyout(app, h, anchor, &monitor, edge);
        let window = &app.windows.flyout;
        window.set_repaint_tick(!window.get_repaint_tick());
        match &app.rt.bench {
            Some(_) => backdrop::show_no_activate(h),
            None => backdrop::show_and_activate(h),
        }
    }
    let window = &app.windows.flyout;
    match (by_keyboard, focus_card) {
        (true, _) => window.invoke_focus_card_at(0, true),
        (false, Some(i)) => window.invoke_focus_card_at(i32::try_from(i).unwrap_or(0), false),
        (false, None) => window.invoke_reset_focus(),
    }
    if !was_open {
        replan(app);
    }
}

/// Builds the flyout model from the current evaluation at `now` and sets it; remembers the card
/// to focus (sign-in, R§10) in `Runtime::focus_card`.
fn refresh_flyout_content(app: &App, now: Timestamp) {
    let scale = app
        .rt
        .flyout_monitor
        .get()
        .map_or(1.0, |m| f64::from(m.dpi) / 96.0);
    let built = {
        let st = app.state.borrow();
        let st = &*st;
        st.evaluation.as_ref().map(|eval| {
            let cx = BuildContext {
                text: st.text_context(now),
                settings: &st.settings,
            };
            let model = build_flyout(eval, &st.history, st.refreshing, &cx);
            let (menus, refs) = card_menus(&model, st);
            (model, menus, refs)
        })
    };
    let Some((model, menus, refs)) = built else {
        return;
    };
    set_flyout(&app.windows.flyout, &model, scale, &menus);
    *app.rt.cards.borrow_mut() = refs;
    if let Some(words) = &model.announce {
        app.windows.flyout.set_announcement(words.into());
        app.rt
            .announce
            .start(TimerMode::SingleShot, Duration::from_secs(2), || {
                with_app(|app| app.windows.flyout.set_announcement(SharedString::new()));
            });
    }
    app.rt.focus_card.set(model.focus_card);
}

/// The flyout's height (logical px, F§1.2): content, at most `min(560, space)`, where `space`
/// (device px, `flyout_pos::flyout_max_height`) is the work area less the 12 px margins and less
/// a floating readout the flyout would otherwise cover. Floored to whole logical px so the
/// window, rounded to device px, never exceeds `space`.
fn flyout_height(content: f32, space: i32, dpi: u32) -> f32 {
    let scale = (f64::from(dpi) / 96.0).max(0.5);
    let fits = (f64::from(space) / scale).floor();
    let max = f64::from(FLYOUT_MAX_H).min(fits);
    // `max` is a few hundred logical px.
    content.min(max.max(100.0) as f32)
}

/// The space for the flyout at `anchor` (device px, `flyout_pos::flyout_max_height`), clear of
/// the readout when it floats.
fn flyout_space(app: &App, anchor: Anchor, monitor: &MonitorInfo, edge: TaskbarEdge) -> i32 {
    let width = device(f64::from(FLYOUT_W), f64::from(monitor.dpi) / 96.0);
    flyout_pos::flyout_max_height(anchor, width, monitor, edge, app.rt.readout_floating.get())
}

/// Makes Slint create the flyout's repeated items (the cards) for the current model, so that
/// `content-height` measures them.
///
/// Slint instantiates `for` items lazily, when it renders the window or handles input. The
/// flyout is shown and hidden with Win32 behind Slint's back, so nothing renders it while it is
/// hidden, and cards set just before it opens would measure as an empty list (100 px: header,
/// list padding, footer): the first open showed a flyout without cards. A pointer-exit is input
/// that does exactly that instantiation; otherwise it only clears hover state left from the last
/// time the flyout was open, which a flyout about to open should not keep anyway.
fn instantiate_flyout_items(window: &ui::Flyout) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerExited);
}

/// The flyout's content height (logical px) for the model it holds now, with its cards laid out
/// even when Slint has not rendered them yet ([`instantiate_flyout_items`]).
pub fn flyout_content_height(window: &ui::Flyout) -> f32 {
    instantiate_flyout_items(window);
    window.get_content_height()
}

/// The flyout's height (logical px, [`flyout_height`]) and rectangle (device px) for `content` at
/// `anchor` (F§1.2, F§1.3): clear of the readout when it floats (`floating`).
fn flyout_layout(
    content: f32,
    anchor: Anchor,
    monitor: &MonitorInfo,
    edge: TaskbarEdge,
    floating: Option<Rect>,
) -> (f32, Rect) {
    let scale = f64::from(monitor.dpi) / 96.0;
    let width = device(f64::from(FLYOUT_W), scale);
    let space = flyout_pos::flyout_max_height(anchor, width, monitor, edge, floating);
    let height = flyout_height(content, space, monitor.dpi);
    let size = Size {
        width,
        height: device(f64::from(height), scale),
    };
    (
        height,
        flyout_pos::flyout_rect(anchor, size, monitor, edge, floating),
    )
}

/// While open, places the flyout again for the readout's placement now (F§1.2, F§1.3): a readout
/// that starts floating under it moves the flyout's edge 12 px beyond the readout (and back when
/// it docks again), the height following the space.
fn replace_open_flyout(app: &App) {
    let (Some(h), Some(old), Some(monitor), Some(anchor)) = (
        hwnd(app.rt.flyout_hwnd.get()),
        app.rt.flyout_rect.get(),
        app.rt.flyout_monitor.get(),
        app.rt.flyout_anchor.get(),
    ) else {
        return;
    };
    let window = &app.windows.flyout;
    let edge = app.state.borrow().readout.edge;
    let (height, rect) = flyout_layout(
        window.get_content_height(),
        anchor,
        &monitor,
        edge,
        app.rt.readout_floating.get(),
    );
    if rect == old {
        return;
    }
    window.set_flyout_height(height);
    backdrop::set_rect(h, rect);
    app.rt.flyout_rect.set(Some(rect));
}

/// Sizes and places the flyout (F§1.2, F§1.3) in device px.
fn place_flyout(app: &App, h: HWND, anchor: Anchor, monitor: &MonitorInfo, edge: TaskbarEdge) {
    let window = &app.windows.flyout;
    let scale = f64::from(monitor.dpi) / 96.0;
    // No resize of the previous rectangle while the cards are laid out below.
    app.rt.flyout_rect.set(None);
    app.rt.flyout_anchor.set(Some(anchor));
    let (height, rect) = flyout_layout(
        flyout_content_height(window),
        anchor,
        monitor,
        edge,
        app.rt.readout_floating.get(),
    );
    window.set_flyout_height(height);
    backdrop::set_rect(h, rect);
    // Moving onto a monitor with another DPI makes winit resize the window synchronously
    // (WM_DPICHANGED): place it again with the size for that DPI.
    if (f64::from(window.window().scale_factor()) - scale).abs() > 0.01 {
        backdrop::set_rect(h, rect);
    }
    app.rt.flyout_rect.set(Some(rect));
}

/// A logical length at `scale` in device px, rounded half up (R§0).
fn device(logical: f64, scale: f64) -> i32 {
    let v = (logical * scale + 0.5).floor();
    // Window sizes are far inside i32.
    v.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// Hides the flyout; `deactivated` records the time for the 300 ms toggle debounce (F§1.4).
pub fn hide_flyout(app: &App, deactivated: bool) {
    {
        let mut st = app.state.borrow_mut();
        if !st.flyout.open {
            return;
        }
        st.flyout.open = false;
        st.flyout.opened_by_keyboard = false;
        if deactivated {
            st.flyout.last_auto_hide = Some(Instant::now());
        }
    }
    app.rt.foreground.stop();
    if let Some(h) = hwnd(app.rt.flyout_hwnd.get()) {
        backdrop::hide(h);
    }
    let window = &app.windows.flyout;
    window.invoke_reset_focus();
    // No animation may run in the hidden window (F§1.4 "Hidden: no timers, no redraws"); the
    // next open sets the real state again.
    window.set_refreshing(false);
    app.rt.flyout_rect.set(None);
    replan(app);
}

/// A notice of the flyout's window procedure (already deferred).
fn on_flyout_notice(app: &App, notice: WindowNotice) {
    match notice {
        WindowNotice::Deactivated => on_flyout_deactivated(app),
        WindowNotice::Theme(_) => apply_theme(app),
        WindowNotice::DpiChanged(_) => {}
    }
}

/// F§1.4 "focus lost → hide". A window of our own process taking the foreground (the card's
/// context menu, a Slint popup) does not close the flyout; while it is up, a light poll hides
/// the flyout once the foreground leaves the process.
fn on_flyout_deactivated(app: &App) {
    if !app.state.borrow().flyout.open || app.rt.bench.is_some() {
        return;
    }
    if foreground_is_ours() {
        app.rt
            .foreground
            .start(TimerMode::Repeated, Duration::from_millis(250), || {
                with_app(|app| {
                    let flyout = app.rt.flyout_hwnd.get();
                    // SAFETY: GetForegroundWindow has no preconditions.
                    let fg = unsafe { GetForegroundWindow() };
                    if fg.0 as isize == flyout {
                        app.rt.foreground.stop();
                    } else if !foreground_is_ours() {
                        app.rt.foreground.stop();
                        hide_flyout(app, true);
                    }
                });
            });
        return;
    }
    hide_flyout(app, true);
}

/// The foreground window belongs to this process.
fn foreground_is_ours() -> bool {
    // SAFETY: GetForegroundWindow has no preconditions; it may return a null handle.
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    // SAFETY: `fg` is a window handle (possibly already destroyed, which only makes the call
    // return 0) and `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid == std::process::id()
}

/// Wires the flyout's Slint callbacks to the app.
fn connect_flyout(window: &ui::Flyout) {
    window.on_dismiss(|| with_app(|app| hide_flyout(app, false)));
    // Cards laid out after a render (new or taller content while open): follow the content.
    window.on_content_height_changed(|| {
        with_app(|app| {
            if app.state.borrow().flyout.open {
                resize_open_flyout(app);
            }
        });
    });
    window.on_refresh(|| with_app(|app| refresh_now(app, None)));
    window.on_open_settings(|| with_app(|app| open_dialog(app, DialogKind::Settings)));
    window.on_footer_action(|| with_app(resume));
    window.on_card_refresh(|index| {
        with_app(|app| {
            let source = card_source(app, index);
            refresh_now(app, source.as_ref());
        });
    });
    window.on_open_service_dashboard(|index| with_app(|app| open_usage_page(app, index)));
    window.on_message_action(|index| with_app(|app| on_card_action(app, index)));
    window.on_card_toggle_pin(|card, meter| with_app(|app| toggle_pin(app, card, meter)));
}

/// The source of card `index`.
fn card_source(app: &App, index: i32) -> Option<SourceKey> {
    let i = usize::try_from(index).ok()?;
    app.rt.cards.borrow().get(i).map(|c| c.source.clone())
}

/// F§9 "Enter on a card" / card menu "Open usage page": the service's page in the browser, then
/// hide.
fn open_usage_page(app: &App, index: i32) {
    let Some(source) = card_source(app, index) else {
        return;
    };
    let url = app
        .state
        .borrow()
        .sources
        .get(&source)
        .and_then(|s| s.provider.usage_page());
    if let Some(url) = url {
        if let Err(e) = lastdrop_win::shell::open_url(url) {
            log::warn!("cannot open the usage page: {e}");
        }
        hide_flyout(app, false);
    }
}

/// A message line's link (F§4.3).
fn on_card_action(app: &App, index: i32) {
    let Some(card) = usize::try_from(index)
        .ok()
        .and_then(|i| app.rt.cards.borrow().get(i).cloned())
    else {
        return;
    };
    match card.action {
        Some(CardAction::Retry) => refresh_now(app, Some(&card.source)),
        Some(CardAction::Resume) => resume(app),
        Some(CardAction::OpenSettings) => open_dialog(app, DialogKind::Settings),
        Some(CardAction::OpenSignIn) => {
            let command = {
                let st = app.state.borrow();
                st.sources
                    .get(&card.source)
                    .and_then(|s| s.provider.sign_in_command(&s.source))
            };
            if let Some(command) = command {
                if let Err(e) = lastdrop_win::shell::open_console(
                    &command.program,
                    &command.args,
                    &command.env,
                    command.cwd.as_deref(),
                ) {
                    log::warn!("cannot open the sign-in console: {e}");
                }
                hide_flyout(app, false);
            }
        }
        None => {}
    }
}

/// Card menu "Show in taskbar readout ▸ meter": pins or unpins it (R§4.3, at most two, in the
/// order picked; no pins left → auto).
fn toggle_pin(app: &App, card: i32, meter: i32) {
    let key = usize::try_from(card)
        .ok()
        .zip(usize::try_from(meter).ok())
        .and_then(|(c, m)| app.rt.cards.borrow().get(c)?.pins.get(m).cloned());
    let Some(key) = key else {
        return;
    };
    {
        let mut st = app.state.borrow_mut();
        let readout = &mut st.settings.readout;
        let mut pins = match &readout.content {
            ReadoutContent::Pinned(p) => p.clone(),
            ReadoutContent::Auto => Vec::new(),
        };
        toggle_in(&mut pins, key);
        readout.content = if pins.is_empty() {
            ReadoutContent::Auto
        } else {
            ReadoutContent::Pinned(pins)
        };
    }
    save_settings(app);
    rebuild(app, Timestamp::now());
}

/// Adds `key` to the pins (dropping the oldest beyond two) or removes it.
pub fn toggle_in(pins: &mut Vec<MeterKey>, key: MeterKey) {
    if let Some(i) = pins.iter().position(|k| *k == key) {
        pins.remove(i);
    } else {
        pins.push(key);
        while pins.len() > lastdrop_core::consts::AUTO_ROWS {
            pins.remove(0);
        }
    }
}

/// The card menus and card references of a flyout model.
fn card_menus(model: &FlyoutModel, st: &AppState) -> (Vec<CardMenu>, Vec<CardRef>) {
    let pins = match &st.settings.readout.content {
        ReadoutContent::Pinned(p) => p.as_slice(),
        ReadoutContent::Auto => &[],
    };
    model
        .cards
        .iter()
        .map(|card| {
            let usage = st
                .sources
                .get(&card.source)
                .and_then(|s| s.provider.usage_page())
                .is_some();
            let menu = card_menu(card, &st.strings, pins, usage);
            let reference = CardRef {
                source: card.source.clone(),
                action: card
                    .message
                    .as_ref()
                    .and_then(|m| m.action.as_ref())
                    .map(|l| l.action),
                pins: card_meter_keys(card).into_iter().map(|(k, _)| k).collect(),
            };
            (menu, reference)
        })
        .unzip()
}

/// The meters of a card with their names (rows, or the balance).
fn card_meter_keys(card: &CardModel) -> Vec<(MeterKey, String)> {
    match &card.body {
        CardBody::Meters(rows) => rows
            .iter()
            .map(|r| (r.key.clone(), r.name.clone()))
            .collect(),
        CardBody::Balance(b) => {
            let name = catalog(b.key.provider)
                .iter()
                .find(|d| d.id == b.key.meter.0)
                .map_or_else(|| b.key.meter.0.clone(), |d| d.name.to_owned());
            vec![(b.key.clone(), name)]
        }
    }
}

/// The card menu of one card (F§9): `Refresh {service}`, the usage page, the pin checklist.
pub fn card_menu(
    card: &CardModel,
    strings: &dyn Strings,
    pins: &[MeterKey],
    has_usage_page: bool,
) -> CardMenu {
    CardMenu {
        refresh_label: text(strings, Msg::MenuCardRefresh, &[("service", &card.name)]),
        has_usage_page,
        pins: card_meter_keys(card)
            .into_iter()
            .map(|(key, name)| (name, pins.contains(&key)))
            .collect(),
    }
}

/// Sets the flyout's fixed labels (`Msg::MenuCard*`).
pub fn set_flyout_labels(window: &ui::Flyout, strings: &dyn Strings) {
    window.set_menu_usage_label(strings.template(Msg::MenuCardUsage).into());
    window.set_menu_readout_label(strings.template(Msg::MenuCardReadout).into());
}

// ---- Models → windows -----------------------------------------------------------------------------

/// Evaluates at `now` and pushes every model: readout (`Arc` to the readout thread), tray icon
/// image and tooltip (only when changed), hover card and flyout (only while visible).
pub fn rebuild(app: &App, now: Timestamp) {
    // Evaluate exactly once per pass: the evaluation's live changes (R§11 announcements, F§10,
    // window resets for notifications, N§1) are consumed by the surfaces built from it.
    let (readout, notification) = {
        let mut st = app.state.borrow_mut();
        st.evaluate(now);
        let notification = st.take_notification(now);
        // A meter seen for the first time, and what notifications announced, are remembered
        // across restarts (state file).
        if let Some(bytes) = st.state_to_save() {
            app.writer
                .handle()
                .replace(app.paths.state.clone(), bytes, "state");
        }
        let st = &*st;
        let Some(eval) = st.evaluation.as_ref() else {
            return;
        };
        let cx = BuildContext {
            text: st.text_context(now),
            settings: &st.settings,
        };
        (Arc::new(build_readout(eval, &cx)), notification)
    };
    let (flyout_open, hover) = {
        let mut st = app.state.borrow_mut();
        // The readout's rows can change with the data, not only with its placement.
        let form = st.readout.form;
        st.readout.rows = shown_rows(&st, Some(&readout), form);
        let st = &*st;
        let hover = st.evaluation.as_ref().and_then(|eval| {
            let cx = BuildContext {
                text: st.text_context(now),
                settings: &st.settings,
            };
            app.rt
                .hover
                .get()
                .map(|(anchor, _)| build_hover_card(eval, anchor, &st.readout.rows, &cx))
        });
        (st.flyout.open, hover)
    };
    app.readout.update(readout.clone());
    *app.rt.last_readout.borrow_mut() = Some(readout);
    update_tray(app, false);
    if flyout_open {
        refresh_flyout_content(app, now);
        resize_open_flyout(app);
    }
    if let (Some(model), Some((_, rect))) = (hover, app.rt.hover.get()) {
        place_hover_card(app, &model, rect);
    }
    if let Some(notification) = notification {
        show_notification(app, &notification);
    }
}

/// Shows a notification from the tray icon (N§4). A failure (the icon is not in the notification
/// area yet) is logged; the notice stays announced and is not retried.
fn show_notification(app: &App, notification: &NotificationModel) {
    let icon = if notification.warning {
        NoticeIcon::Warning
    } else {
        NoticeIcon::Info
    };
    if let Err(e) = app
        .tray
        .notify(&notification.title, &notification.body, icon)
    {
        log::warn!("notification not shown: {e}");
    }
}

/// While open, a height change resizes the flyout with the bottom edge anchored (F§1.2).
fn resize_open_flyout(app: &App) {
    let (Some(h), Some(rect), Some(monitor)) = (
        hwnd(app.rt.flyout_hwnd.get()),
        app.rt.flyout_rect.get(),
        app.rt.flyout_monitor.get(),
    ) else {
        return;
    };
    let window = &app.windows.flyout;
    let scale = f64::from(monitor.dpi) / 96.0;
    let edge = app.state.borrow().readout.edge;
    let anchor = app.rt.flyout_anchor.get().unwrap_or(Anchor::Rect(rect));
    let space = flyout_space(app, anchor, &monitor, edge);
    let height = flyout_height(window.get_content_height(), space, monitor.dpi);
    let device_h = device(f64::from(height), scale);
    if device_h == rect.height() {
        return;
    }
    window.set_flyout_height(height);
    let new = if edge == TaskbarEdge::Top {
        Rect {
            bottom: rect.top + device_h,
            ..rect
        }
    } else {
        Rect {
            top: rect.bottom - device_h,
            ..rect
        }
    };
    backdrop::set_rect(h, new);
    app.rt.flyout_rect.set(Some(new));
}

/// Rebuilds the tray icon image and tooltip from the current evaluation when they changed (or
/// always with `force`: theme change, Explorer restart).
fn update_tray(app: &App, force: bool) {
    let now = Timestamp::now();
    let (model, dpi, theme) = {
        let st = app.state.borrow();
        let Some(eval) = st.evaluation.as_ref() else {
            return;
        };
        let cx = BuildContext {
            text: st.text_context(now),
            settings: &st.settings,
        };
        // SAFETY: GetDpiForSystem has no preconditions.
        let dpi = st
            .readout
            .monitor
            .map_or_else(|| unsafe { GetDpiForSystem() }, |m| m.dpi);
        (build_tray(eval, &cx), dpi, st.theme)
    };
    let previous = app.rt.last_tray.borrow().clone();
    let image_changed = force
        || previous
            .as_ref()
            .is_none_or(|(m, d, t)| m.icon != model.icon || *d != dpi || *t != theme);
    let tip_changed = force
        || previous
            .as_ref()
            .is_none_or(|(m, _, _)| m.tooltip != model.tooltip);
    if image_changed {
        match tray_icon_image(&model, dpi, &theme, &app.render) {
            Ok(image) => {
                if let Err(e) = app.tray.set_image(&image) {
                    log::warn!("tray icon: {e}");
                }
            }
            Err(e) => log::warn!("tray icon image: {e}"),
        }
    }
    if tip_changed && let Err(e) = app.tray.set_tooltip(&model.tooltip) {
        log::warn!("tray tooltip: {e}");
    }
    *app.rt.last_tray.borrow_mut() = Some((model, dpi, theme));
}

/// Shows the hover card for `anchor` over `rect` (R§8): builds the model, sets the window,
/// places it with `lastdrop_win::flyout_pos::hover_card_rect`, `show_no_activate`.
pub fn show_hover_card(app: &App, anchor: HoverAnchor, rect: Rect) {
    if app.state.borrow().flyout.open || app.rt.hover_hwnd.get() == 0 {
        // Opening the flyout hides the card; while it is open the card stays away (R§10).
        return;
    }
    app.rt.hover.set(Some((anchor, rect)));
    // Builds and places the card with every other surface from one evaluation.
    rebuild(app, Timestamp::now());
    if let Some(h) = hwnd(app.rt.hover_hwnd.get()) {
        backdrop::show_no_activate(h);
        // The card's live region: screen readers hear it as it opens (R§11; hide clears it, so
        // the same card shown again is heard again).
        let card = &app.windows.hover_card;
        card.set_announcement(card.get_uia_name());
    }
}

/// Sets the hover card's content, fits it into the space above (below) the taskbar by dropping
/// "Also tracked" lines from the bottom, then the sentence (R§8.1), and places it.
fn place_hover_card(app: &App, model: &HoverCardModel, anchor: Rect) {
    let Some(h) = hwnd(app.rt.hover_hwnd.get()) else {
        return;
    };
    let monitor = flyout_pos::monitor_for(Anchor::Rect(anchor));
    let scale = f64::from(monitor.dpi) / 96.0;
    let window = &app.windows.hover_card;
    set_hover_card(window, model, scale);
    let (taskbar, edge) = taskbar_of(app, &monitor);
    let gap = f64::from(CARD_TASKBAR_GAP) * scale;
    let margin = f64::from(CARD_EDGE_MARGIN) * scale;
    // The card sits beyond the taskbar, or beyond a readout floating off it (never over it).
    let space_device = match edge {
        TaskbarEdge::Top => {
            f64::from(monitor.work.bottom)
                - margin
                - (f64::from(taskbar.bottom.max(anchor.bottom)) + gap)
        }
        _ => f64::from(taskbar.top.min(anchor.top)) - gap - (f64::from(monitor.work.top) + margin),
    };
    let space = (space_device / scale) as f32;
    let mut visible = window.get_also_visible();
    let also = model.also.as_ref().map(|a| (a.lines.len(), a.more_count));
    loop {
        if window.get_content_height() <= space {
            break;
        }
        if visible > 0 {
            visible -= 1;
            window.set_also_visible(visible);
            // The dropped lines join `+N more` (R§8.2).
            if let Some((lines, more)) = also {
                let shown = usize::try_from(visible).unwrap_or(0);
                let hidden = more + lines.saturating_sub(shown);
                let words = if hidden > 0 {
                    let st = app.state.borrow();
                    text(
                        &st.strings,
                        Msg::CardAlsoMore,
                        &[("n", &hidden.to_string())],
                    )
                } else {
                    String::new()
                };
                window.set_also_more(words.into());
            }
        } else if window.get_show_sentence() {
            window.set_show_sentence(false);
        } else {
            break;
        }
    }
    let height = window.get_content_height();
    window.set_card_height(height);
    let size = Size {
        width: device(f64::from(CARD_W), scale),
        height: device(f64::from(height), scale),
    };
    let rect = flyout_pos::hover_card_rect(anchor, size, &monitor, taskbar, edge);
    backdrop::set_rect(h, rect);
    // A full repaint after every placement (and so before every show): Win32 moved or resized
    // the card, or will show it, without Slint knowing, and Slint presents only what it saw
    // change.
    window.set_repaint_tick(!window.get_repaint_tick());
    // As for the flyout: moving onto a monitor with another DPI makes winit resize the window
    // synchronously (WM_DPICHANGED) with the old monitor's scale; place it again.
    if (f64::from(window.window().scale_factor()) - scale).abs() > 0.01 {
        backdrop::set_rect(h, rect);
    }
}

/// The taskbar of `monitor`: as the readout reported it, else the strip between the work area
/// and the monitor's bottom edge.
fn taskbar_of(app: &App, monitor: &MonitorInfo) -> (Rect, TaskbarEdge) {
    let st = app.state.borrow();
    if st
        .readout
        .monitor
        .is_some_and(|m| m.monitor == monitor.monitor)
    {
        return (st.readout.taskbar, st.readout.edge);
    }
    let (m, w) = (monitor.monitor, monitor.work);
    if w.top > m.top {
        (Rect { bottom: w.top, ..m }, TaskbarEdge::Top)
    } else {
        (Rect { top: w.bottom, ..m }, TaskbarEdge::Bottom)
    }
}

/// Hides the hover card.
pub fn hide_hover_card(app: &App) {
    if app.rt.hover.take().is_none() {
        return;
    }
    if let Some(h) = hwnd(app.rt.hover_hwnd.get()) {
        backdrop::hide(h);
    }
    // The next show starts from the full content again, and is announced again.
    app.windows.hover_card.set_show_sentence(true);
    app.windows.hover_card.set_announcement(SharedString::new());
}

// ---- Model → Slint conversions (geometry in device px as floats, F§11) ---------------------------

/// A Slint model from a vector.
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// Device px as a Slint float.
fn px(v: i32) -> f32 {
    v as f32
}

/// `BarGeometry` → `BarGeom` (`dashed` false; [`meter_data`] and the hover card set it from the
/// bar input).
pub fn bar_geom(geometry: &BarGeometry) -> ui::BarGeom {
    let segs = |segs: &[lastdrop_core::readout::bar::Seg]| {
        model(
            segs.iter()
                .map(|s| ui::Seg {
                    x: px(s.x0),
                    w: px(s.x1 - s.x0),
                })
                .collect(),
        )
    };
    ui::BarGeom {
        track: segs(&geometry.track),
        fill: segs(&geometry.fill),
        tick_x: px(geometry.tick.unwrap_or(0)),
        has_tick: geometry.tick.is_some(),
        hair: px(geometry.hair),
        fill_h: px(geometry.fill_h),
        tick_h: px(geometry.tick_h),
        y_track: px(geometry.y_track),
        height: px(geometry.height),
        round_start: geometry.round_start,
        round_end: geometry.round_end,
        dashed: false,
    }
}

/// Core tone → Slint tone.
pub fn tone(t: Tone) -> ui::Tone {
    match t {
        Tone::Calm => ui::Tone::Calm,
        Tone::Caution => ui::Tone::Caution,
        Tone::Critical => ui::Tone::Critical,
        Tone::Stale => ui::Tone::Stale,
    }
}

/// A bar's fill role → Slint tone (Caution → caution, Critical → critical, Stale → stale, else
/// calm).
fn role_tone(role: ColorRole) -> ui::Tone {
    match role {
        ColorRole::Caution => ui::Tone::Caution,
        ColorRole::Critical => ui::Tone::Critical,
        ColorRole::Stale => ui::Tone::Stale,
        _ => ui::Tone::Calm,
    }
}

/// Core severity glyph → Slint glyph (`None` → `Glyph.none`). Used for the flyout verdict, the
/// hover card header and the "Also tracked" time column.
pub fn severity_glyph(glyph: Option<SeverityGlyph>) -> ui::Glyph {
    match glyph {
        None => ui::Glyph::None,
        Some(SeverityGlyph::Warning) => ui::Glyph::Warning,
        Some(SeverityGlyph::Critical) => ui::Glyph::Critical,
        Some(SeverityGlyph::Success) => ui::Glyph::Success,
        Some(SeverityGlyph::Info) => ui::Glyph::Info,
        Some(SeverityGlyph::Stale) => ui::Glyph::Stale,
    }
}

/// A provider glyph for Slint's `ProviderGlyph`: the R Appendix A paths of
/// `lastdrop_core::glyph::provider_paths` joined into one command string with the first path's
/// stroke width (grid units; the paths of one glyph differ by ≤ 0.05) and round caps or joins
/// when any path has them, or a monogram's letters.
pub fn glyph_data(glyph: &ProviderGlyph) -> ui::GlyphData {
    match glyph {
        ProviderGlyph::Drawn(provider) => {
            let paths = provider_paths(*provider);
            ui::GlyphData {
                commands: paths
                    .iter()
                    .map(|p| p.d)
                    .collect::<Vec<_>>()
                    .join(" ")
                    .into(),
                stroke: paths.first().map_or(1.0, |p| p.stroke_width),
                round_caps: paths.iter().any(|p| p.round_caps),
                round_joins: paths.iter().any(|p| p.round_joins),
                monogram: SharedString::new(),
            }
        }
        ProviderGlyph::Monogram(letters) => ui::GlyphData {
            commands: SharedString::new(),
            stroke: 0.0,
            round_caps: false,
            round_joins: false,
            monogram: letters.into(),
        },
    }
}

/// A sparkline at `scale` (`lastdrop_core::flyout::spark_geometry`).
pub fn spark_data(spark: &SparkModel, scale: f64) -> ui::SparkData {
    let g = spark_geometry(spark, scale);
    ui::SparkData {
        cols: model(
            g.columns
                .iter()
                .map(|c| ui::SparkCol {
                    x: px(c.x),
                    h: px(c.h),
                    today: c.today,
                })
                .collect(),
        ),
        dim: spark.dim,
        a11y: spark.a11y.as_str().into(),
        box_w: px(g.box_w),
        box_h: px(g.box_h),
        col_w: px(g.col_w),
        hair: px(g.hair),
        base_x: px(g.baseline.x0),
        base_w: px(g.baseline.x1 - g.baseline.x0),
    }
}

/// A meter row at `scale` (`bar_geometry` with W 310, rowH 9). `right.kind` Reset/Say/Plain →
/// `right-kind` 0/1/2; a Say glyph Warning → `Tone.caution`, Critical → `Tone.critical`;
/// `line: None` → `""`; `pct: None` (LOADING) → 0 (the label already says it is reading).
pub fn meter_data(row: &MeterRowModel, scale: f64) -> ui::MeterData {
    let mut bar = bar_geom(&bar_geometry(
        &row.bar,
        FLYOUT_BAR_W,
        FLYOUT_BAR_ROW_H,
        scale,
    ));
    bar.dashed = row.bar.dashed;
    ui::MeterData {
        name: row.name.as_str().into(),
        value: row.value.as_str().into(),
        value_dim: row.value_dim,
        right_kind: match row.right.kind {
            RightKind::Reset => 0,
            RightKind::Say => 1,
            RightKind::Plain => 2,
        },
        right_glyph: match row.right.glyph {
            Some(SeverityGlyph::Warning) => ui::Tone::Caution,
            Some(SeverityGlyph::Critical) => ui::Tone::Critical,
            _ => ui::Tone::Calm,
        },
        right_text: row.right.text.as_str().into(),
        line: row.line.as_deref().unwrap_or_default().into(),
        tone: tone(row.tone),
        bar,
        a11y: row.a11y.as_str().into(),
        pct: row.pct.map_or(0, i32::from),
    }
}

/// `MessageKind` → Slint `msg-kind` (its position + 1; 0 = no message).
fn message_kind(kind: MessageKind) -> i32 {
    match kind {
        MessageKind::Offline => 1,
        MessageKind::NoData => 2,
        MessageKind::SignIn => 3,
        MessageKind::Paused => 4,
        MessageKind::FirstFetchFailed => 5,
        MessageKind::ApiKey => 6,
    }
}

/// A card at `scale`: `plan: None` → `""`; `message` → `msg-kind` (`MessageKind` position + 1,
/// 0 = none), `msg-lead`, `msg-text`, `msg-action` (link label); `CardBody::Balance` →
/// `is-balance`, `amount`, `unit`, `forecast`, `forecast-tone` (Warning → caution, Critical →
/// critical, else calm), `burn` (= `spent`), `until`, `balance-a11y`; the card menu from `menu`.
pub fn card_data(card: &CardModel, scale: f64, menu: &CardMenu) -> ui::CardData {
    let (msg_kind, msg_lead, msg_text, msg_action) = match &card.message {
        None => (0, "", "", ""),
        Some(m) => (
            message_kind(m.kind),
            m.lead.as_str(),
            m.text.as_str(),
            m.action.as_ref().map_or("", |a| a.label.as_str()),
        ),
    };
    let mut data = ui::CardData {
        glyph: glyph_data(&card.glyph),
        name: card.name.as_str().into(),
        plan: card.plan.as_deref().unwrap_or_default().into(),
        age: card.age.as_str().into(),
        msg_kind,
        msg_lead: msg_lead.into(),
        msg_text: msg_text.into(),
        msg_action: msg_action.into(),
        meters: model(Vec::new()),
        is_balance: false,
        amount: SharedString::new(),
        unit: SharedString::new(),
        forecast: SharedString::new(),
        forecast_tone: ui::Tone::Calm,
        burn: SharedString::new(),
        until: SharedString::new(),
        balance_a11y: SharedString::new(),
        spark: spark_data(&card.spark, scale),
        a11y: card.a11y.as_str().into(),
        menu_refresh: menu.refresh_label.as_str().into(),
        has_usage_page: menu.has_usage_page,
        pins: model(
            menu.pins
                .iter()
                .map(|(name, pinned)| ui::PinData {
                    name: name.as_str().into(),
                    pinned: *pinned,
                })
                .collect(),
        ),
    };
    match &card.body {
        CardBody::Meters(rows) => {
            data.meters = model(rows.iter().map(|r| meter_data(r, scale)).collect());
        }
        CardBody::Balance(b) => {
            data.is_balance = true;
            data.amount = b.amount.as_str().into();
            data.unit = b.unit.as_str().into();
            data.forecast = b.forecast.as_str().into();
            data.forecast_tone = match b.forecast_glyph {
                Some(SeverityGlyph::Warning) => ui::Tone::Caution,
                Some(SeverityGlyph::Critical) => ui::Tone::Critical,
                _ => ui::Tone::Calm,
            };
            data.burn = b.spent.as_str().into();
            data.until = b.until.as_deref().unwrap_or_default().into();
            data.balance_a11y = b.a11y.as_str().into();
        }
    }
    data
}

/// Sets every flyout property from the model at the window's `scale`; `menus` holds one
/// [`CardMenu`] per card (missing ones are empty).
pub fn set_flyout(window: &ui::Flyout, model: &FlyoutModel, scale: f64, menus: &[CardMenu]) {
    let empty = CardMenu::default();
    window.set_title_text(model.title.as_str().into());
    window.set_verdict_text(model.verdict.text.as_str().into());
    window.set_verdict_glyph(severity_glyph(model.verdict.glyph));
    window.set_refreshing(model.refreshing);
    window.set_refresh_name(model.refresh_name.as_str().into());
    window.set_refresh_tooltip(model.refresh_tooltip.as_str().into());
    window.set_settings_label(model.settings_label.as_str().into());
    let (footer_text, footer_link) = match &model.footer_note {
        Some(note) => (note.text.as_str(), note.link.label.as_str()),
        None => ("", ""),
    };
    window.set_footer_text(footer_text.into());
    window.set_footer_link(footer_link.into());
    let cards: Vec<ui::CardData> = model
        .cards
        .iter()
        .enumerate()
        .map(|(i, card)| card_data(card, scale, menus.get(i).unwrap_or(&empty)))
        .collect();
    // Replace the rows in place when the count is unchanged so focus and scroll survive a
    // refresh while the flyout is open.
    let current = window.get_cards();
    match current.as_any().downcast_ref::<VecModel<ui::CardData>>() {
        Some(existing) if existing.row_count() == cards.len() => {
            for (i, card) in cards.into_iter().enumerate() {
                existing.set_row_data(i, card);
            }
        }
        _ => window.set_cards(model_rc(cards)),
    }
}

/// A `VecModel` as a `ModelRc` (kept downcastable for in-place updates).
fn model_rc<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

/// One hover-card meter block at `scale` (bars W 296, rowH 11, calm fill TextPrimary).
fn card_meter_data(meter: &CardMeter, scale: f64) -> ui::CardMeterData {
    let bar = |input: &lastdrop_core::readout::bar::BarInput| {
        let mut g = bar_geom(&bar_geometry(input, CARD_BAR_W, CARD_BAR_ROW_H, scale));
        g.dashed = input.dashed;
        g
    };
    match meter {
        CardMeter::Window(w) => ui::CardMeterData {
            is_balance: false,
            name: w.name.as_str().into(),
            value: w.value.as_str().into(),
            value_semibold: w.value_semibold,
            bar: bar(&w.bar),
            tone: role_tone(w.bar.fill),
            elapsed_line: w.elapsed_line.as_str().into(),
            forecast: w.forecast.as_deref().unwrap_or_default().into(),
            forecast_semibold: w.forecast_semibold,
            reset_line: w.reset_line.as_str().into(),
            reset_in: w.reset_in.as_deref().unwrap_or_default().into(),
            left_line: SharedString::new(),
            rate_line: SharedString::new(),
            until: SharedString::new(),
        },
        CardMeter::Balance(b) => ui::CardMeterData {
            is_balance: true,
            name: b.name.as_str().into(),
            value: b.value.as_str().into(),
            value_semibold: b.value_semibold,
            bar: bar(&b.bar),
            tone: role_tone(b.bar.fill),
            elapsed_line: SharedString::new(),
            forecast: b.forecast.as_str().into(),
            forecast_semibold: b.forecast_semibold,
            reset_line: SharedString::new(),
            reset_in: SharedString::new(),
            left_line: b.left_line.as_str().into(),
            rate_line: b.rate_line.as_deref().unwrap_or_default().into(),
            until: b.until.as_deref().unwrap_or_default().into(),
        },
    }
}

/// One "Also tracked" line.
fn also_line_data(line: &AlsoLine) -> ui::AlsoLineData {
    ui::AlsoLineData {
        glyph: glyph_data(&line.glyph),
        label: line.label.as_str().into(),
        value: line.value.as_str().into(),
        value_semibold: line.value_semibold,
        time_glyph: match line.time.glyph {
            None => ui::Glyph::None,
            Some(GlyphKind::Icon(Icon::Reset)) => ui::Glyph::Reset,
            Some(GlyphKind::Icon(_)) => ui::Glyph::None,
            Some(GlyphKind::Severity(s)) => severity_glyph(Some(s)),
        },
        time_text: line.time.text.as_str().into(),
        time_semibold: line.time.semibold,
        time_dim: !matches!(
            line.time.role,
            ColorRole::TextPrimary | ColorRole::AccentText
        ),
        time_link: line.time.role == ColorRole::AccentText,
    }
}

/// Sets every hover card property from the model at the window's `scale` (bars W 296, rowH 11,
/// calm fill TextPrimary). A meter block's `tone` is its bar's fill role (Caution → caution,
/// Critical → critical, Stale → stale, else calm). Shows everything (`also-visible` = every
/// line, the sentence on); the window then drops lines that do not fit (R§8.1).
pub fn set_hover_card(window: &ui::HoverCard, model: &HoverCardModel, scale: f64) {
    window.set_header_glyph(severity_glyph(model.header.glyph));
    window.set_title_text(model.header.title.as_str().into());
    window.set_sentence(model.sentence.as_str().into());
    window.set_show_sentence(true);
    window.set_services(model_rc(
        model
            .services
            .iter()
            .map(|s| ui::CardServiceData {
                glyph: glyph_data(&s.glyph),
                name: s.name.as_str().into(),
                plan: s.plan.as_deref().unwrap_or_default().into(),
                age: s.age.as_str().into(),
                meters: model_rc(s.meters.iter().map(|m| card_meter_data(m, scale)).collect()),
            })
            .collect(),
    ));
    match &model.also {
        Some(also) => {
            window.set_also_header(also.header.as_str().into());
            window.set_also(model_rc(also.lines.iter().map(also_line_data).collect()));
            window.set_also_visible(i32::try_from(also.lines.len()).unwrap_or(i32::MAX));
            window.set_also_more(also.more.as_deref().unwrap_or_default().into());
        }
        None => {
            window.set_also_header(SharedString::new());
            window.set_also(model_rc(Vec::new()));
            window.set_also_visible(0);
            window.set_also_more(SharedString::new());
        }
    }
    window.set_uia_name(model.uia_name.as_str().into());
}

/// An ARGB colour as a Slint colour.
fn color(c: Argb) -> slint::Color {
    slint::Color::from_argb_u8(c.a(), c.r(), c.g(), c.b())
}

/// Sets a window's `Theme` global from the theme (each window has its own global instance). The
/// flyout and the hover card follow the Windows mode (`SystemUsesLightTheme`, F§1.1, R§0).
pub fn set_theme(global: &ui::Theme<'_>, theme: &ThemeInfo) {
    global.set_dark(theme.taskbar_dark);
    global.set_high_contrast(theme.high_contrast.is_some());
    global.set_accent_fill(color(palette::resolve(ColorRole::AccentFill, theme)));
    global.set_accent_text(color(palette::resolve(ColorRole::AccentText, theme)));
    if let Some(hc) = theme.high_contrast {
        global.set_hc_window(color(hc.window));
        global.set_hc_window_text(color(hc.window_text));
        global.set_hc_highlight(color(hc.highlight));
        global.set_hc_highlight_text(color(hc.highlight_text));
        global.set_hc_gray_text(color(hc.gray_text));
        global.set_hc_hotlight(color(hc.hotlight));
    }
}

/// The tray icon image for `model` at the taskbar's `dpi` (`lastdrop_win::trayicon::render`).
pub fn tray_icon_image(
    model: &TrayModel,
    dpi: u32,
    theme: &ThemeInfo,
    render: &RenderFactory,
) -> Result<TrayImage, AppError> {
    let size = lastdrop_win::trayicon::icon_size(dpi);
    Ok(lastdrop_win::trayicon::render(model, size, theme, render)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lastdrop_core::grammar::English;
    use lastdrop_core::model::{AccountKey, MeterId, ProviderId};

    #[test]
    fn minute_tick_is_the_next_boundary_plus_250_ms() {
        let tz = TimeZone::fixed(jiff::tz::offset(2));
        let now: Timestamp = "2026-10-10T10:42:30.5Z".parse().expect("valid");
        let next = next_minute_tick(now, &tz);
        assert_eq!(next.to_string(), "2026-10-10T10:43:00.25Z");
        // Exactly on a boundary: the next one.
        let on: Timestamp = "2026-10-10T10:42:00Z".parse().expect("valid");
        assert_eq!(
            next_minute_tick(on, &tz).to_string(),
            "2026-10-10T10:43:00.25Z"
        );
    }

    #[test]
    fn tray_menu_follows_the_state() {
        let menu = TrayMenu::build(&English, &MenuState::default());
        let labels: Vec<String> = menu
            .items
            .iter()
            .map(|i| match i {
                MenuItem::Command { label, .. } | MenuItem::Submenu { label, .. } => label.clone(),
                MenuItem::Separator => "-".to_owned(),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Refresh now",
                "Pause updates",
                "Taskbar readout",
                "-",
                "Settings…",
                "About Lastdrop",
                "-",
                "Quit Lastdrop"
            ]
        );
        assert_eq!(menu.command(1), Some(MenuCommand::Refresh));
        assert!(menu.ids.iter().all(|(id, _)| *id != 0));
        let ids: BTreeSet<u32> = menu.ids.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids.len(), menu.ids.len(), "ids are unique");

        let paused = TrayMenu::build(
            &English,
            &MenuState {
                paused: true,
                readout_on: false,
                readout_pinned: false,
            },
        );
        assert!(paused.ids.iter().any(|(_, c)| *c == MenuCommand::Resume));
        assert!(!paused.ids.iter().any(|(_, c)| *c == MenuCommand::PauseHour));
        let off_checked = paused.items.iter().any(|i| match i {
            MenuItem::Submenu { items, .. } => items.iter().any(
                |s| matches!(s, MenuItem::Command { label, checked: true, .. } if label == "Off"),
            ),
            _ => false,
        });
        assert!(off_checked);
    }

    #[test]
    fn a_reload_ignores_a_missing_or_broken_file_and_keeps_the_pause() {
        let current = Settings {
            pause: Some(Pause { until: None }),
            ..Settings::default()
        };
        let mut saved = Settings {
            start_with_windows: true,
            ..Settings::default()
        };
        saved.readout.enabled = false;

        let loaded = reloaded_settings(&current, Ok((saved.clone(), LoadOutcome::Loaded)))
            .expect("a clean load applies");
        assert_eq!(
            loaded,
            Settings {
                pause: current.pause,
                ..saved.clone()
            }
        );
        assert!(
            reloaded_settings(
                &current,
                Ok((saved.clone(), LoadOutcome::Migrated { from: 0 }))
            )
            .is_some()
        );
        assert_eq!(
            reloaded_settings(&current, Ok((Settings::default(), LoadOutcome::Missing))),
            None,
            "no file: keep the current settings"
        );
        assert_eq!(
            reloaded_settings(
                &current,
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "not json"
                ))
            ),
            None,
            "a broken file never brings the defaults"
        );
        assert_eq!(
            reloaded_settings(
                &current,
                Ok((
                    Settings::default(),
                    LoadOutcome::Corrupt {
                        backup: std::path::PathBuf::from("settings.json.bad-1")
                    }
                ))
            ),
            None
        );
        assert_eq!(
            reloaded_settings(
                &current,
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            ),
            None
        );
    }

    #[test]
    fn pause_is_active_until_its_end() {
        let now: Timestamp = "2026-10-10T10:42:00Z".parse().expect("valid");
        let mut s = Settings::default();
        assert!(!pause_active(&s, now));
        s.pause = Some(Pause { until: None });
        assert!(pause_active(&s, now));
        s.pause = Some(Pause {
            until: Some("2026-10-10T10:43:00Z".parse().expect("valid")),
        });
        assert!(pause_active(&s, now));
        s.pause = Some(Pause {
            until: Some("2026-10-10T10:42:00Z".parse().expect("valid")),
        });
        assert!(!pause_active(&s, now));
    }

    #[test]
    fn pins_keep_at_most_two_in_the_order_picked() {
        let key = |m: &str| MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::new(m),
        };
        let mut pins = Vec::new();
        toggle_in(&mut pins, key("a"));
        toggle_in(&mut pins, key("b"));
        toggle_in(&mut pins, key("c"));
        assert_eq!(pins, [key("b"), key("c")]);
        toggle_in(&mut pins, key("b"));
        assert_eq!(pins, [key("c")]);
    }

    #[test]
    fn glyphs_join_their_paths() {
        let codex = glyph_data(&ProviderGlyph::Drawn(ProviderId::Codex));
        assert!(codex.commands.contains("M3.35 4.35"));
        assert!(codex.commands.starts_with("M3.2 1.4H8.8"));
        assert!((codex.stroke - 1.05).abs() < 1e-6);
        assert!(codex.round_caps && codex.round_joins);
        let mono = glyph_data(&ProviderGlyph::Monogram("Cu".to_owned()));
        assert_eq!(mono.monogram.as_str(), "Cu");
        assert!(mono.commands.is_empty());
    }

    #[test]
    fn device_rounding_is_half_up() {
        assert_eq!(device(360.0, 1.25), 450);
        assert_eq!(device(478.0, 1.5), 717);
        assert_eq!(device(55.0, 1.5), 83);
        assert_eq!(device(0.5, 1.0), 1);
    }

    #[test]
    fn flyout_height_is_capped_by_the_space() {
        let monitor = |h: i32, dpi: u32| MonitorInfo {
            monitor: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: h + 48,
            },
            work: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: h,
            },
            dpi,
        };
        let icon = Anchor::Rect(Rect {
            left: 1780,
            top: 1040,
            right: 1812,
            bottom: 1072,
        });
        let space = |m: &MonitorInfo, avoid: Option<Rect>| {
            let width = device(f64::from(FLYOUT_W), f64::from(m.dpi) / 96.0);
            flyout_pos::flyout_max_height(icon, width, m, TaskbarEdge::Bottom, avoid)
        };
        let full = monitor(1032, 96);
        assert_eq!(flyout_height(478.0, space(&full, None), 96), 478.0);
        assert_eq!(flyout_height(700.0, space(&full, None), 96), 560.0);
        // A 500 px work area: 500 − 24.
        let short = monitor(500, 96);
        assert_eq!(flyout_height(700.0, space(&short, None), 96), 476.0);
        // At 150 % the work area is 1032 / 1.5 = 688 logical px.
        let scaled = monitor(1032, 144);
        assert_eq!(flyout_height(700.0, space(&scaled, None), 144), 560.0);
        // A readout floating 4 px above the taskbar, under the flyout: the flyout ends 12 px above
        // it, so a 500 px work area leaves 500 − 4 − 40 − 24 = 432.
        let floating = Rect {
            left: 1620,
            top: 456,
            right: 1796,
            bottom: 496,
        };
        assert_eq!(
            flyout_height(700.0, space(&short, Some(floating)), 96),
            432.0
        );
        // At 125 % a space of 701 device px is 560.8 logical px: floored, so the window (rounded
        // to device px) cannot grow past the space.
        assert_eq!(flyout_height(700.0, 701, 120), 560.0);
        assert_eq!(flyout_height(700.0, 601, 120), 480.0);
        assert!(device(f64::from(flyout_height(700.0, 601, 120)), 1.25) <= 601);

        // The layout an open flyout is placed again with when the readout starts floating
        // under it: its bottom edge moves 12 px above the readout, and it gets shorter.
        let (height, docked) = flyout_layout(700.0, icon, &short, TaskbarEdge::Bottom, None);
        assert_eq!((height, docked.bottom), (476.0, 488));
        let (height, clear) =
            flyout_layout(700.0, icon, &short, TaskbarEdge::Bottom, Some(floating));
        assert_eq!((height, clear.bottom), (432.0, floating.top - 12));
        assert!(clear.top >= short.work.top + 12);
    }
}
