//! Renders the tray icon image from a `TrayModel` (R§9): BGRA at the size of
//! `GetSystemMetricsForDpi(SM_CXSMICON, dpi)` of the taskbar's monitor (16/20/24/32; in-between
//! sizes use the next smaller layout, centred — never a scaled bitmap). The app hands the image to
//! [`crate::tray::Tray::set_image`] on the main thread.
//!
//! Threading: runs on the main thread with its own [`RenderFactory`] (the tray icon belongs to
//! the main thread, ARCHITECTURE "Threads").

use lastdrop_core::color::ColorRole;
use lastdrop_core::glyph::DROP_PATH;
use lastdrop_core::readout::bar::tray_bar_geometry;
use lastdrop_core::readout::tray::{TrayLayout, TrayModel, tray_layout_for};
use lastdrop_core::readout::{FontFace, FontWeight, TrayIcon};
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

use crate::geom::{PointF, RectF};
use crate::palette::{Argb, resolve};
use crate::render::{
    BarColors, Canvas, Font, PathPaint, RenderFactory, TextMeasurer, TextPlacement,
};
use crate::theme::ThemeInfo;

/// The small-icon size for a monitor DPI: `GetSystemMetricsForDpi(SM_CXSMICON, dpi)` (16 at
/// 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %).
pub fn icon_size(dpi: u32) -> u32 {
    // SAFETY: a plain query without pointers.
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    match u32::try_from(size) {
        Ok(s) if s > 0 => s,
        // Not expected; the documented relation at the standard DPIs.
        _ => (16 * dpi.max(96)).div_ceil(96),
    }
}

/// The drop, scaled about the centre of the `key × key` layout by `factor` (R§9.2: × 0.95
/// filled for LOADING, × 0.78 stroked in the EXHAUSTED tile). Returns the path origin and scale.
fn drop_transform(key: f32, offset: f32, factor: f32) -> (PointF, f32) {
    let scale = key / 16.0 * factor;
    let origin = PointF {
        x: offset + key / 2.0 - 8.0 * scale,
        y: offset + key / 2.0 - 8.0 * scale,
    };
    (origin, scale)
}

/// Draws `model.icon` at `size × size` px for `theme` (taskbar theme; contrast themes per R§9.2) with the
/// R§9.2 layout for that size (`lastdrop_core::readout::tray::tray_layout_for`): digits as one
/// DirectWrite layout with letter spacing, origin `x = key/2 + 0.25 − w/2` unsnapped; the mini bar
/// from `tray_bar_geometry`; tiles; the drop path. Returns **straight** (non-premultiplied) BGRA
/// rows, top-down, as [`crate::tray::Tray::set_image`] expects.
pub fn render(
    model: &TrayModel,
    size: u32,
    theme: &ThemeInfo,
    factory: &RenderFactory,
) -> windows::core::Result<crate::tray::TrayImage> {
    let size = size.max(1);
    let (layout, offset) = tray_layout_for(size);
    let mut canvas = Canvas::new(factory, size, size)?;
    let measurer = TextMeasurer::new(factory);
    canvas.begin(Argb::TRANSPARENT);
    draw(
        &mut canvas,
        &measurer,
        &model.icon,
        layout,
        offset as f32,
        theme,
    );
    canvas.end()?;
    let mut bgra = canvas.pixels()?;
    unpremultiply(&mut bgra);
    Ok(crate::tray::TrayImage { size, bgra })
}

/// The icon's pixels for one layout at `offset` (R§9.1 shape ladder, R§9.2 geometry).
fn draw(
    canvas: &mut Canvas,
    measurer: &TextMeasurer,
    icon: &TrayIcon,
    layout: &TrayLayout,
    offset: f32,
    theme: &ThemeInfo,
) {
    let color = |role: ColorRole| resolve(role, theme);
    let key = layout.key as f32;
    let digits = |canvas: &mut Canvas, text: &str, font_px: f32, baseline: f32, role: ColorRole| {
        let font = Font {
            face: FontFace::UiSmall,
            size_px: font_px,
            weight: FontWeight::Semibold,
        };
        // `w = Σ advances + n·ls` with the trailing spacing after every digit (R§9.2).
        let w = measurer.advance_font(text, &font, layout.letter_spacing);
        let origin = PointF {
            x: offset + key / 2.0 + 0.25 - w / 2.0,
            y: offset + baseline,
        };
        canvas.text(
            text,
            &font,
            TextPlacement::Baseline { origin },
            layout.letter_spacing,
            color(role),
        );
    };
    let tile_rect = RectF {
        x: offset,
        y: offset,
        w: key,
        h: key,
    };
    match icon {
        TrayIcon::Digits {
            digits: text,
            role,
            bar,
        } => {
            digits(
                canvas,
                text,
                layout.digits_font,
                layout.digits_baseline,
                *role,
            );
            let geometry = tray_bar_geometry(bar, layout);
            let colors = BarColors {
                fill: color(bar.fill),
                track: color(bar.track),
                tick: color(bar.tick),
            };
            let origin = PointF {
                x: offset + layout.bar_x as f32,
                y: offset,
            };
            canvas.bar(&geometry, origin, colors);
        }
        TrayIcon::CautionTile { digits: text } => {
            // Stroke centred on a rect inset by half the stroke.
            let sw = layout.tile_outline;
            let inset = RectF {
                x: offset + sw / 2.0,
                y: offset + sw / 2.0,
                w: key - sw,
                h: key - sw,
            };
            canvas.stroke_rounded_rect(inset, layout.tile_radius, sw, color(ColorRole::Caution));
            digits(
                canvas,
                text,
                layout.tile_digits_font,
                layout.tile_digits_baseline,
                ColorRole::TextPrimary,
            );
        }
        TrayIcon::CriticalTile { digits: text } => {
            canvas.fill_rounded_rect(tile_rect, layout.tile_radius, color(ColorRole::Critical));
            digits(
                canvas,
                text,
                layout.tile_digits_font,
                layout.tile_digits_baseline,
                ColorRole::TextOnAccent,
            );
        }
        TrayIcon::ExhaustedTile => {
            canvas.fill_rounded_rect(tile_rect, layout.tile_radius, color(ColorRole::Critical));
            let (origin, scale) = drop_transform(key, offset, 0.78);
            canvas.path(
                DROP_PATH,
                origin,
                scale,
                PathPaint::Stroke {
                    color: color(ColorRole::TextOnAccent),
                    width: layout.drop_stroke,
                    round_caps: false,
                    round_joins: false,
                },
            );
        }
        TrayIcon::Drop => {
            let (origin, scale) = drop_transform(key, offset, 0.95);
            canvas.path(
                DROP_PATH,
                origin,
                scale,
                PathPaint::Fill(color(ColorRole::TextPrimary)),
            );
        }
    }
}

/// Premultiplied BGRA → straight BGRA, in place.
fn unpremultiply(bgra: &mut [u8]) {
    for p in bgra.as_chunks_mut::<4>().0 {
        let a = p[3];
        if a == 0 {
            *p = [0, 0, 0, 0];
            continue;
        }
        let un = |c: u8| -> u8 {
            let v = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
            u8::try_from(v.min(255)).unwrap_or(255)
        };
        *p = [un(p[0]), un(p[1]), un(p[2]), a];
    }
}

#[cfg(test)]
mod tests {
    use lastdrop_core::readout::bar::{BarInput, BarMode};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    use super::*;
    use crate::theme::AccentShades;

    fn factory() -> RenderFactory {
        // SAFETY: initializes COM on the test thread (S_FALSE when already initialized is fine).
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        RenderFactory::new().expect("factories")
    }

    fn theme(dark: bool) -> ThemeInfo {
        let accent = Argb(0xFF8E_3AA7);
        ThemeInfo {
            taskbar_dark: dark,
            apps_dark: dark,
            accent: AccentShades {
                accent,
                dark1: accent,
                dark2: accent,
                light2: Argb(0xFFD5_8AE8),
                light3: Argb(0xFFD5_8AE8),
            },
            transparency: true,
            high_contrast: None,
        }
    }

    fn model(icon: TrayIcon) -> TrayModel {
        TrayModel {
            icon,
            tooltip: "Lastdrop".to_owned(),
        }
    }

    fn ok_digits(text: &str, used: f64, elapsed: Option<f64>) -> TrayIcon {
        TrayIcon::Digits {
            digits: text.to_owned(),
            role: ColorRole::TextPrimary,
            bar: BarInput {
                used,
                elapsed,
                mode: BarMode::Normal,
                dashed: false,
                fill: ColorRole::TextPrimary,
                tick: ColorRole::TextPrimary,
                track: ColorRole::Track,
            },
        }
    }

    /// The pixel at `(x, y)` as `[r, g, b, a]`.
    fn rgba(img: &crate::tray::TrayImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.size + x) * 4) as usize;
        [
            img.bgra[i + 2],
            img.bgra[i + 1],
            img.bgra[i],
            img.bgra[i + 3],
        ]
    }

    #[test]
    fn premultiplied_pixels_become_straight() {
        let mut px = [0, 0, 0, 0, 64, 32, 128, 128, 255, 255, 255, 255, 9, 9, 9, 0];
        unpremultiply(&mut px);
        assert_eq!(
            px,
            [
                0, 0, 0, 0, 128, 64, 255, 128, 255, 255, 255, 255, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn sizes_follow_the_system_metric() {
        assert_eq!(icon_size(96), 16);
        assert_eq!(icon_size(120), 20);
        assert_eq!(icon_size(144), 24);
        assert_eq!(icon_size(192), 32);
    }

    #[test]
    fn ok_icon_has_digits_over_the_mini_bar_at_16() {
        // R§9.2 at 16: bar x 1, length 14, track y 13, fill y 12 (3 high), tick y 11 (5 high).
        // u .42, e .5533 → f 6, k 8: fill [0,6), track [6,7) [8,9) [10,14).
        let f = factory();
        let img = render(
            &model(ok_digits("42", 0.42, Some(0.5533))),
            16,
            &theme(true),
            &f,
        )
        .expect("render");
        assert_eq!(img.size, 16);
        assert_eq!(img.bgra.len(), 16 * 16 * 4);
        // Fill: opaque white (dark taskbar TextPrimary #FFFFFFFF), columns 1..7, rows 12..15.
        assert_eq!(rgba(&img, 1, 13), [255, 255, 255, 255]);
        assert_eq!(rgba(&img, 6, 12), [255, 255, 255, 255]);
        // Gap after the fill: column 1 + 7 = 8 is the left gap, empty on the track row.
        assert_eq!(rgba(&img, 8, 13)[3], 0);
        // Tick at column 1 + 8 = 9, rows 11..16.
        assert_eq!(rgba(&img, 9, 11), [255, 255, 255, 255]);
        assert_eq!(rgba(&img, 9, 15), [255, 255, 255, 255]);
        // Track (dark: #8BFFFFFF) at column 1 + 12, row 13.
        assert_eq!(rgba(&img, 13, 13), [255, 255, 255, 0x8B]);
        // Digits above the bar: some ink in rows 1..10.
        assert!((1..10).any(|y| (0..16).any(|x| rgba(&img, x, y)[3] > 128)));
        // Nothing in the last row.
        assert!((0..16).all(|x| rgba(&img, x, 15)[3] == 0 || x == 9));
    }

    #[test]
    fn tiles_fill_or_outline_the_icon_square() {
        let f = factory();
        let crit = render(
            &model(TrayIcon::CriticalTile {
                digits: "94".to_owned(),
            }),
            16,
            &theme(false),
            &f,
        )
        .expect("render");
        // Light Critical #FFC42B1C fills the square except the rounded corners.
        assert_eq!(rgba(&crit, 8, 1), [0xC4, 0x2B, 0x1C, 255]);
        assert!(rgba(&crit, 0, 0)[3] < 255);
        let warn = render(
            &model(TrayIcon::CautionTile {
                digits: "78".to_owned(),
            }),
            16,
            &theme(false),
            &f,
        )
        .expect("render");
        // Outline 1 px on the edge (Caution #FF9D5D00), empty inside near the edge.
        assert_eq!(rgba(&warn, 8, 0), [0x9D, 0x5D, 0x00, 255]);
        assert_eq!(rgba(&warn, 8, 1)[3], 0);
    }

    #[test]
    fn in_between_sizes_are_centred_and_drops_draw() {
        let f = factory();
        let img = render(&model(TrayIcon::ExhaustedTile), 28, &theme(true), &f).expect("render");
        assert_eq!(img.size, 28);
        // The 24 layout with a 2 px margin: the margin stays empty, the tile starts at 2.
        assert_eq!(rgba(&img, 14, 0)[3], 0);
        assert_eq!(rgba(&img, 14, 1)[3], 0);
        assert_eq!(rgba(&img, 14, 2)[3], 255);
        assert_eq!(rgba(&img, 14, 25)[3], 255);
        assert_eq!(rgba(&img, 14, 26)[3], 0);
        let drop = render(&model(TrayIcon::Drop), 32, &theme(true), &f).expect("render");
        // Filled drop, white on transparent: the round bottom is solid, the corners empty.
        assert_eq!(rgba(&drop, 16, 20), [255, 255, 255, 255]);
        assert_eq!(rgba(&drop, 1, 1)[3], 0);
    }
}
