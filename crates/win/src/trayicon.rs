//! Renders the tray icon image from a `TrayModel` (R§9): RGBA at the size of
//! `GetSystemMetricsForDpi(SM_CXSMICON, dpi)` of the taskbar's monitor (16/20/24/32; in-between
//! sizes use the next smaller layout, centred — never a scaled bitmap). The app hands the image to
//! [`crate::tray::Tray::set_image`] on the main thread.
//!
//! Threading: runs on the main thread with its own [`RenderFactory`] (the tray icon belongs to
//! the main thread, ARCHITECTURE "Threads").
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use lastdrop_core::readout::tray::TrayModel;

use crate::render::RenderFactory;
use crate::theme::ThemeInfo;

/// The small-icon size for a monitor DPI: `GetSystemMetricsForDpi(SM_CXSMICON, dpi)` (16 at
/// 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %).
pub fn icon_size(dpi: u32) -> u32 {
    todo!("M1: GetSystemMetricsForDpi(SM_CXSMICON)")
}

/// Draws `model.icon` at `size × size` px for `theme` (taskbar theme; contrast themes per R§9.2) with the
/// R§9.2 layout for that size (`lastdrop_core::readout::tray::tray_layout_for`): digits as one
/// DirectWrite layout with letter spacing, origin `x = key/2 + 0.25 − w/2` unsnapped; the mini bar
/// from `tray_bar_geometry`; tiles; the drop path. Returns **straight** (non-premultiplied) RGBA
/// rows, top-down, as [`crate::tray::Tray::set_image`] expects.
pub fn render(
    model: &TrayModel,
    size: u32,
    theme: &ThemeInfo,
    factory: &RenderFactory,
) -> windows::core::Result<crate::tray::TrayImage> {
    todo!("M1: R§9 tray icon")
}
