//! The readout on the secondary taskbars (docs/design/every-taskbar-spec.md): with "Show it on
//! every taskbar" on, one more readout thread per `Shell_SecondaryTrayWnd`, each drawing the same
//! model as the primary one. The primary readout thread says when the taskbars may have changed
//! (`SystemEvent::Taskbars`); [`sync`] then starts the missing ones and stops the gone ones.
//!
//! A secondary readout reports only its own pointer input and placement: a click opens the
//! flyout at that readout, a hover shows the hover card above it, a right click the tray menu.
//! Its placement never changes what the primary one reported (tray icon size, the flyout's
//! floating readout, R§9.1, F§1.3).
//!
//! Threading: main thread only.

use std::cell::Cell;
use std::sync::Arc;

use lastdrop_core::readout::ReadoutModel;
use lastdrop_core::readout::hovercard::HoverAnchor;
use lastdrop_win::flyout_pos::Anchor;
use lastdrop_win::geom::Rect;
use lastdrop_win::placement::Placement;
use lastdrop_win::readout::{
    ReadoutConfig, ReadoutEvent, ReadoutHandle, TaskbarTarget, secondary_monitors,
};
use lastdrop_win::theme::ThemeInfo;

use crate::wiring::{self, App};

/// One secondary taskbar's readout.
pub struct SecondaryReadout {
    /// The monitor of its taskbar (device px): what it is known by.
    pub monitor: Rect,
    /// Its thread.
    handle: ReadoutHandle,
    /// Where it is drawn now; `None` while hidden.
    rect: Cell<Option<Rect>>,
}

/// The monitors that should have a readout: every one with a secondary taskbar, when the readout
/// is on and "every taskbar" is chosen; none otherwise.
fn wanted(app: &App) -> Vec<Rect> {
    let every = {
        let st = app.state.borrow();
        app.readout_allowed(&st.settings) && st.settings.readout.every_taskbar
    };
    if every {
        secondary_monitors()
    } else {
        Vec::new()
    }
}

/// Starts a readout for every secondary taskbar that should have one and stops the others
/// (every-taskbar-spec §2). Cheap when nothing changed: one walk of the top-level windows of
/// `Shell_SecondaryTrayWnd`.
pub fn sync(app: &App) {
    let wanted = wanted(app);
    let gone: Vec<SecondaryReadout> = {
        let mut list = app.secondary.borrow_mut();
        let (keep, gone) = std::mem::take(&mut *list)
            .into_iter()
            .partition(|r| wanted.contains(&r.monitor));
        *list = keep;
        gone
    };
    for readout in gone {
        log::info!("readout: stopping the one on a secondary taskbar");
        readout.handle.shutdown();
    }
    let missing: Vec<Rect> = {
        let list = app.secondary.borrow();
        wanted
            .into_iter()
            .filter(|m| !list.iter().any(|r| r.monitor == *m))
            .collect()
    };
    if missing.is_empty() {
        return;
    }
    let theme = app.state.borrow().theme;
    let model = app.rt.last_readout.borrow().clone();
    for monitor in missing {
        match spawn(monitor, theme) {
            Ok(handle) => {
                log::info!("readout: started on a secondary taskbar");
                if let Some(model) = &model {
                    handle.update(Arc::clone(model));
                }
                app.secondary.borrow_mut().push(SecondaryReadout {
                    monitor,
                    handle,
                    rect: Cell::new(None),
                });
            }
            Err(e) => log::warn!("readout: not started on a secondary taskbar: {e}"),
        }
    }
}

/// A readout thread for the secondary taskbar on `monitor`; its events come back through the
/// event loop to [`on_event`].
fn spawn(monitor: Rect, theme: ThemeInfo) -> std::io::Result<ReadoutHandle> {
    ReadoutHandle::spawn(
        ReadoutConfig {
            enabled: true,
            hover_delay: None,
            theme,
            target: TaskbarTarget::Secondary { monitor },
        },
        Box::new(move |event| {
            let _ = slint::invoke_from_event_loop(move || {
                wiring::with_app(|app| on_event(app, monitor, event));
            });
        }),
    )
}

/// Hands every secondary readout the model the primary one got.
pub fn update(app: &App, model: &Arc<ReadoutModel>) {
    for readout in app.secondary.borrow().iter() {
        readout.handle.update(Arc::clone(model));
    }
}

/// Hands every secondary readout the theme.
pub fn set_theme(app: &App, theme: ThemeInfo) {
    for readout in app.secondary.borrow().iter() {
        readout.handle.set_theme(theme);
    }
}

/// Stops every secondary readout (shutdown).
pub fn stop_all(app: &App) {
    let all = std::mem::take(&mut *app.secondary.borrow_mut());
    for readout in all {
        readout.handle.shutdown();
    }
}

/// An event of the readout on the taskbar of `monitor` (R§10, every-taskbar-spec §2).
fn on_event(app: &App, monitor: Rect, event: ReadoutEvent) {
    let rect = || {
        app.secondary
            .borrow()
            .iter()
            .find(|r| r.monitor == monitor)
            .and_then(|r| r.rect.get())
    };
    match event {
        ReadoutEvent::Hover { rect } => wiring::show_hover_card(app, HoverAnchor::Readout, rect),
        ReadoutEvent::Leave => wiring::hide_hover_card(app),
        ReadoutEvent::Click | ReadoutEvent::Invoked => {
            wiring::hide_hover_card(app);
            let anchor = rect().map_or_else(|| wiring::tray_anchor(app), Anchor::Rect);
            if matches!(event, ReadoutEvent::Click) {
                wiring::toggle_flyout(app, anchor, false);
            } else {
                wiring::open_flyout(app, anchor, true);
            }
        }
        ReadoutEvent::RightClick { pt } => {
            wiring::hide_hover_card(app);
            wiring::show_tray_menu(app, pt);
        }
        ReadoutEvent::PlacementChanged { placement, .. } => {
            let shown = match placement {
                Placement::Docked { rect, .. } | Placement::Floating { rect, .. } => Some(rect),
                Placement::Hidden(_) => None,
            };
            if let Some(r) = app.secondary.borrow().iter().find(|r| r.monitor == monitor) {
                r.rect.set(shown);
            }
        }
        // The primary readout forwards the system broadcasts; a secondary one sends none.
        ReadoutEvent::System(_) => {}
    }
}
