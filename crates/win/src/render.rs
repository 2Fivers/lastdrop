//! Drawing: Direct2D on a WIC bitmap (software render target), DirectWrite text with
//! **grayscale** antialiasing (ClearType cannot be used on per-pixel-alpha surfaces), premultiplied
//! BGRA, pixel-snapped glyph runs. The same engine as the taskbar clock (ARCHITECTURE "Drawing",
//! R§6.1, R§6.5).
//!
//! Used by the readout thread (the readout bitmap, pushed with `UpdateLayeredWindow`) and by the
//! tray icon renderer. A [`RenderFactory`] and every [`Canvas`] belong to the thread that created
//! them (COM objects; not `Send`).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use lastdrop_core::readout::bar::BarGeometry;
use lastdrop_core::readout::{FontFace, FontWeight, TextMeasure, TextStyle};
use windows::Win32::Graphics::Direct2D::{ID2D1Factory, ID2D1RenderTarget};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;
use windows::Win32::Graphics::Imaging::{IWICBitmap, IWICImagingFactory};

use crate::geom::{PointF, RectF};
use crate::palette::Argb;

/// Font families with fallbacks (R§6.5: Segoe UI on Windows 10, Segoe MDL2 Assets for icons;
/// F§8: fall back to Segoe UI Variable Text if a named instance does not resolve).
pub const fn family(face: FontFace) -> &'static [&'static str] {
    match face {
        FontFace::UiSmall => &[
            "Segoe UI Variable Small",
            "Segoe UI Variable Text",
            "Segoe UI",
        ],
        FontFace::UiText => &["Segoe UI Variable Text", "Segoe UI"],
        FontFace::UiDisplay => &[
            "Segoe UI Variable Display",
            "Segoe UI Variable Text",
            "Segoe UI",
        ],
        FontFace::Icons => &["Segoe Fluent Icons", "Segoe MDL2 Assets"],
    }
}

/// A font at a device size: em size `style.size × s` px (not rounded, R§0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    /// Family role.
    pub face: FontFace,
    /// Em size in device px.
    pub size_px: f32,
    /// Weight.
    pub weight: FontWeight,
}

impl Font {
    /// `style` at `scale`.
    pub fn scaled(style: TextStyle, scale: f64) -> Font {
        Font {
            face: style.face,
            size_px: (f64::from(style.size) * scale) as f32,
            weight: style.weight,
        }
    }
}

/// Horizontal alignment of text in its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    /// Left edge at the box's x.
    Left,
    /// Right edge at the box's right (the VALUE box, R§5.7).
    Right,
    /// Centred.
    Center,
}

/// How a run of text is placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextPlacement {
    /// In a line box: DirectWrite line box of `rect.h` with the font's default line spacing, text
    /// vertically centred in it, as the taskbar clock does (R§6.1). Pixel-snapped.
    LineBox {
        /// The box.
        rect: RectF,
        /// Horizontal alignment.
        align: Align,
    },
    /// On a baseline at a fractional origin, not snapped (`D2D1_DRAW_TEXT_OPTIONS_NO_SNAP`): the
    /// tray digits (R§9.2).
    Baseline {
        /// Left end of the baseline.
        origin: PointF,
    },
}

/// Shared factories of one thread: Direct2D (single-threaded), DirectWrite (shared), WIC.
pub struct RenderFactory {
    /// Direct2D factory.
    pub d2d: ID2D1Factory,
    /// DirectWrite factory.
    pub dwrite: IDWriteFactory,
    /// WIC factory.
    pub wic: IWICImagingFactory,
}

impl RenderFactory {
    /// Creates the factories (COM must be initialized on the thread).
    pub fn new() -> windows::core::Result<RenderFactory> {
        todo!("M1: create D2D/DWrite/WIC factories")
    }
}

/// A premultiplied-BGRA bitmap with a software Direct2D render target.
pub struct Canvas {
    #[expect(
        dead_code,
        reason = "used once the drawing methods are implemented (M1)"
    )]
    bitmap: IWICBitmap,
    #[expect(
        dead_code,
        reason = "used once the drawing methods are implemented (M1)"
    )]
    target: ID2D1RenderTarget,
    width: u32,
    height: u32,
}

/// How a path is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathPaint {
    /// Filled.
    Fill(Argb),
    /// Stroked.
    Stroke {
        /// Colour.
        color: Argb,
        /// Width in device px.
        width: f32,
        /// Round caps.
        round_caps: bool,
        /// Round joins.
        round_joins: bool,
    },
}

/// Colours of a pace bar (resolved from the `BarInput` roles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BarColors {
    /// Fill.
    pub fill: Argb,
    /// Track.
    pub track: Argb,
    /// Tick.
    pub tick: Argb,
}

impl Canvas {
    /// A transparent canvas of `width × height` device px.
    pub fn new(factory: &RenderFactory, width: u32, height: u32) -> windows::core::Result<Canvas> {
        todo!(
            "M1: WIC bitmap (GUID_WICPixelFormat32bppPBGRA) + CreateWicBitmapRenderTarget (software), grayscale text AA"
        )
    }

    /// Width in device px.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in device px.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Starts a frame, clearing to `clear` (the readout clears to the hit-test colour
    /// `0x01000000`, R§6.1).
    pub fn begin(&mut self, clear: Argb) {
        todo!("M1: BeginDraw + Clear")
    }

    /// Ends the frame.
    pub fn end(&mut self) -> windows::core::Result<()> {
        todo!("M1: EndDraw")
    }

    /// A filled axis-aligned rectangle (bar segments, ticks: exact device pixels, aliased).
    pub fn fill_rect(&mut self, rect: RectF, color: Argb) {
        todo!("M1: FillRectangle (aliased)")
    }

    /// A filled rounded rectangle (hover plate radius 4, tray tiles, rounded fill ends).
    pub fn fill_rounded_rect(&mut self, rect: RectF, radius: f32, color: Argb) {
        todo!("M1: FillRoundedRectangle")
    }

    /// A stroked rounded rectangle, the stroke centred on `rect` (outlined tray tile, high
    /// contrast plate outline, monogram box).
    pub fn stroke_rounded_rect(&mut self, rect: RectF, radius: f32, width: f32, color: Argb) {
        todo!("M1: DrawRoundedRectangle")
    }

    /// A line.
    pub fn line(&mut self, from: PointF, to: PointF, width: f32, color: Argb) {
        todo!("M1: DrawLine")
    }

    /// Text with `font`, placed per `placement`; `letter_spacing` is added after every character
    /// (`IDWriteTextLayout1::SetCharacterSpacing`, R§9.2), 0 elsewhere.
    pub fn text(
        &mut self,
        text: &str,
        font: &Font,
        placement: TextPlacement,
        letter_spacing: f32,
        color: Argb,
    ) {
        todo!("M1: DirectWrite text layout + DrawTextLayout")
    }

    /// One Segoe Fluent Icons glyph at `size_px`, centred in `cell` (R§6.5 glyph cells; severity
    /// glyphs draw their two layers with two calls).
    pub fn glyph(&mut self, ch: char, size_px: f32, cell: RectF, color: Argb) {
        todo!("M1: icon glyph")
    }

    /// An SVG path (provider glyphs, the drop) scaled by `scale` and moved to `origin`.
    pub fn path(&mut self, svg_d: &str, origin: PointF, scale: f32, paint: PathPaint) {
        todo!("M1: parse SVG path data into an ID2D1PathGeometry")
    }

    /// A pace bar at `origin` (the bar's row box top-left) with the geometry of
    /// `lastdrop_core::readout::bar_geometry`: track and tick aliased, fill segments with rounded
    /// outer ends where `round_start`/`round_end` say (radius `fill_h / 2`), dashes square.
    pub fn bar(&mut self, geometry: &BarGeometry, origin: PointF, colors: BarColors) {
        todo!("M1: R§6.6 drawing")
    }

    /// The pixels, premultiplied BGRA, top-down, stride `width × 4`.
    pub fn pixels(&self) -> windows::core::Result<Vec<u8>> {
        todo!("M1: IWICBitmap::Lock + copy")
    }
}

/// Measures text with DirectWrite for `lastdrop_core::readout::layout::compute_layout` (R§6.3).
pub struct TextMeasurer {
    #[expect(dead_code, reason = "used once advance() is implemented (M1)")]
    dwrite: IDWriteFactory,
}

impl TextMeasurer {
    /// A measurer over the thread's DirectWrite factory.
    pub fn new(factory: &RenderFactory) -> TextMeasurer {
        TextMeasurer {
            dwrite: factory.dwrite.clone(),
        }
    }
}

impl TextMeasure for TextMeasurer {
    fn advance(&self, text: &str, style: TextStyle, scale: f64) -> f32 {
        todo!("M1: IDWriteTextLayout metrics width at size × scale")
    }
}
