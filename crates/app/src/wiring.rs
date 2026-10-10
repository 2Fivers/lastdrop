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
//! - Slint callbacks of the flyout, hover card, settings and about windows;
//! - the minute tick: a `slint::Timer` at the next local minute boundary + 250 ms (R§12), re-armed
//!   after every tick, resume and time change;
//! - the refresh timer: one `slint::Timer` for the scheduler's next wake-up.
//!
//! On each new reading and each minute tick: evaluate (`lastdrop_core::stats::evaluate`), then
//! build the readout model (handed to the readout thread as an `Arc`), the tray model (icon image +
//! tooltip), and — only while visible — the hover card and the flyout (F§1.4: "Hidden: no timers,
//! no redraws").
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::cell::RefCell;
use std::rc::Rc;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use lastdrop_core::flyout::{CardModel, FlyoutModel, MeterRowModel, SparkModel, Tone};
use lastdrop_core::glyph::{ProviderGlyph, SeverityGlyph};
use lastdrop_core::grammar::Strings;
use lastdrop_core::readout::bar::BarGeometry;
use lastdrop_core::readout::hovercard::HoverCardModel;
use lastdrop_core::readout::{HoverAnchor, TrayModel};
use lastdrop_core::settings::Settings;
use lastdrop_win::flyout_pos::Anchor;
use lastdrop_win::geom::Rect;
use lastdrop_win::hotkey::HotkeyRegistration;
use lastdrop_win::readout::{ReadoutEvent, ReadoutHandle};
use lastdrop_win::render::RenderFactory;
use lastdrop_win::single_instance::{InstanceGuard, Listener};
use lastdrop_win::theme::ThemeInfo;
use lastdrop_win::tray::{MenuItem, Tray, TrayImage, TrayInput};

use crate::persist::Paths;
use crate::scheduler::{JobResult, Scheduler};
use crate::state::AppState;
use crate::{AppError, Args, ui};

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

/// Runs the app until "Quit Lastdrop": DPI awareness (the manifest's per-monitor v2), dark menus,
/// theme and locale, history, the Slint backend (winit + software renderer, the spike's window
/// attributes: not active, no taskbar entry, no decorations, always on top, off-screen, acrylic,
/// round corners), the windows created hidden and kept alive, the tray icon, the readout thread,
/// the workers, the timers; then the event loop; then an orderly shutdown (tray icon removed,
/// readout and workers joined, history flushed).
pub fn run(startup: Startup) -> Result<(), AppError> {
    todo!("M1: start-up and event loop")
}

/// The Slint windows, created hidden at start-up and kept alive (F§1.1: "open in a few ms").
pub struct Windows {
    /// The flyout.
    pub flyout: ui::Flyout,
    /// The hover card.
    pub hover_card: ui::HoverCard,
    /// The settings window, created on first use.
    pub settings: Option<ui::SettingsWindow>,
    /// The about window, created on first use.
    pub about: Option<ui::AboutWindow>,
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
    /// Taskbar readout ▸ Pinned meters… (opens settings at the pins).
    ReadoutPinned,
    /// Taskbar readout ▸ Off.
    ReadoutOff,
    /// Settings…
    Settings,
    /// About Lastdrop (required by Slint's licence).
    About,
    /// Quit Lastdrop.
    Quit,
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
    /// Builds the R§10 menu with localized labels (`Msg::Menu*`).
    pub fn build(strings: &dyn Strings) -> TrayMenu {
        todo!("M1: R§10 tray menu")
    }

    /// The command of a menu event's id.
    pub fn command(&self, id: u32) -> Option<MenuCommand> {
        self.ids.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
    }
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
    /// The tray menu ids.
    pub menu: TrayMenu,
    /// Direct2D/DirectWrite/WIC of the main thread (tray icon images).
    pub render: RenderFactory,
    /// The second-instance listener.
    pub listener: Listener,
    /// The global hotkey, when set in settings (R§10).
    pub hotkey: Option<HotkeyRegistration>,
}

/// A refresh result arrived: apply it, record history, rebuild the models, re-plan the timer.
pub fn on_job_result(app: &App, result: JobResult) {
    todo!("M1: job result")
}

/// The minute tick (R§12): recompute `N` and severities (a WARNING can turn CRITICAL by runway
/// between readings), then freshness, then redraw; trim history memory; re-arm.
pub fn on_minute_tick(app: &App) {
    todo!("M1: minute tick")
}

/// A readout event (R§10): hover → hover card after the delay; leave → hide (100 ms grace);
/// click → toggle the flyout (sign-in card focused when a service needs sign-in); UIA invoke →
/// open as a keyboard open; right click → the context menu at the point; placement → remember
/// form/rows/monitor/edge, re-render the tray icon if the DPI changed, rebuild; system → theme
/// (read once, then `ReadoutHandle::set_theme`, tray icon, Slint `Theme` globals), locale,
/// re-arm the minute tick.
pub fn on_readout_event(app: &App, event: ReadoutEvent) {
    todo!("M1: readout events")
}

/// Tray icon input (R§10): `PopupOpen`/`PopupClose` → hover card (`HoverAnchor::TrayIcon`);
/// `Select` → toggle the flyout (300 ms debounce after a deactivation hide, F§1.4); `KeySelect` →
/// open with focus on the first card; `ContextMenu` → `Tray::show_menu` at the point, then
/// [`on_menu_command`]; `Recreated` → re-render and re-apply the icon (the DPI may have changed).
pub fn on_tray_input(app: &App, input: TrayInput) {
    todo!("M1: tray input")
}

/// A tray menu command.
pub fn on_menu_command(app: &App, command: MenuCommand) {
    todo!("M1: menu commands")
}

/// Evaluates at `now` and pushes every model: readout (`Arc` to the readout thread), tray icon
/// image and tooltip (only when changed), hover card and flyout (only while visible).
pub fn rebuild(app: &App, now: Timestamp) {
    todo!("M1: rebuild models")
}

/// Toggles the flyout from `anchor` (F§1.3–F§1.4): position on the anchor's monitor, size from
/// `content-height`, `show_and_activate`, repaint tick; or hide.
pub fn toggle_flyout(app: &App, anchor: Anchor, by_keyboard: bool) {
    todo!("M1: toggle flyout")
}

/// Shows the hover card for `anchor` over `rect` (R§8): builds the model, sets the window,
/// places it with `lastdrop_win::flyout_pos::hover_card_rect`, `show_no_activate`.
pub fn show_hover_card(app: &App, anchor: HoverAnchor, rect: Rect) {
    todo!("M1: show hover card")
}

/// Hides the hover card.
pub fn hide_hover_card(app: &App) {
    todo!("M1: hide hover card")
}

/// The next minute tick: the next local minute boundary after `now` in `tz`, plus
/// `MINUTE_TICK_OFFSET` (250 ms) (R§12).
pub fn next_minute_tick(now: Timestamp, tz: &TimeZone) -> Timestamp {
    todo!("M1: R§12 minute boundary")
}

// ---- Model → Slint conversions (geometry in device px as floats, F§11) ---------------------------

/// `BarGeometry` → `BarGeom`.
pub fn bar_geom(geometry: &BarGeometry) -> ui::BarGeom {
    todo!("M1: BarGeom")
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
/// stroke width (grid units; the paths of one glyph differ by ≤ 0.05), or a monogram's letters.
pub fn glyph_data(glyph: &ProviderGlyph) -> ui::GlyphData {
    todo!("M1: provider glyph data")
}

/// A sparkline at `scale` (`lastdrop_core::flyout::spark_geometry`).
pub fn spark_data(spark: &SparkModel, scale: f64) -> ui::SparkData {
    todo!("M1: SparkData")
}

/// A meter row at `scale` (`bar_geometry` with W 310, rowH 9). `right.kind` Reset/Say/Plain →
/// `right-kind` 0/1/2; a Say glyph Warning → `Tone.caution`, Critical → `Tone.critical`;
/// `line: None` → `""`; `pct: None` (LOADING) → 0 (the label already says it is reading).
pub fn meter_data(row: &MeterRowModel, scale: f64) -> ui::MeterData {
    todo!("M1: MeterData")
}

/// A card at `scale`: `plan: None` → `""`; `message` → `msg-kind` (`MessageKind` position + 1,
/// 0 = none), `msg-lead`, `msg-text`, `msg-action` (link label); `CardBody::Balance` →
/// `is-balance`, `amount`, `unit`, `forecast`, `forecast-tone` (Warning → caution, Critical →
/// critical, else calm), `burn` (= `spent`), `until`, `balance-a11y`.
pub fn card_data(card: &CardModel, scale: f64) -> ui::CardData {
    todo!("M1: CardData")
}

/// Sets every flyout property from the model at the window's `scale`.
pub fn set_flyout(window: &ui::Flyout, model: &FlyoutModel, scale: f64) {
    todo!("M1: set flyout")
}

/// Sets every hover card property from the model at the window's `scale` (bars W 296, rowH 11,
/// calm fill TextPrimary). A meter block's `tone` is its bar's fill role (Caution → caution,
/// Critical → critical, Stale → stale, else calm).
pub fn set_hover_card(window: &ui::HoverCard, model: &HoverCardModel, scale: f64) {
    todo!("M1: set hover card")
}

/// Sets a window's `Theme` global from the theme (each window has its own global instance).
pub fn set_theme(global: &ui::Theme<'_>, theme: &ThemeInfo) {
    todo!("M1: Theme global")
}

/// The tray icon image for `model` at the taskbar's `dpi` (`lastdrop_win::trayicon::render`).
pub fn tray_icon_image(
    model: &TrayModel,
    dpi: u32,
    theme: &ThemeInfo,
    render: &RenderFactory,
) -> Result<TrayImage, AppError> {
    todo!("M1: tray icon image")
}
