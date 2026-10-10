//! Drawing: Direct2D on a WIC bitmap (software render target), DirectWrite text with
//! **grayscale** antialiasing (ClearType cannot be used on per-pixel-alpha surfaces), premultiplied
//! BGRA, pixel-snapped glyph runs. The same engine as the taskbar clock (ARCHITECTURE "Drawing",
//! R§6.1, R§6.5).
//!
//! Used by the readout thread (the readout bitmap, pushed with `UpdateLayeredWindow`) and by the
//! tray icon renderer. A [`RenderFactory`] and every [`Canvas`] belong to the thread that created
//! them (COM objects; not `Send`).

use std::cell::RefCell;

use lastdrop_core::readout::bar::BarGeometry;
use lastdrop_core::readout::{FontFace, FontWeight, TextMeasure, TextStyle};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_WINDING,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND,
    D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_LINE_JOIN_MITER, D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
    ID2D1Factory, ID2D1GeometrySink, ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush,
    ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG_CONTEXTUAL_ALTERNATES,
    DWRITE_FONT_FEATURE_TAG_CONTEXTUAL_LIGATURES, DWRITE_FONT_FEATURE_TAG_KERNING,
    DWRITE_FONT_FEATURE_TAG_STANDARD_LIGATURES, DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_LINE_METRICS,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory,
    IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout, IDWriteTextLayout1,
    IDWriteTypography,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BOOL, HSTRING, Interface, w};
use windows_numerics::Vector2;

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

/// The first family of [`family`]`(face)` that the system font collection has; the last
/// candidate when none resolves (DirectWrite then falls back on its own). Resolved once per
/// thread and face.
pub fn resolve_family(dwrite: &IDWriteFactory, face: FontFace) -> &'static str {
    thread_local! {
        static RESOLVED: RefCell<[Option<&'static str>; 4]> = const { RefCell::new([None; 4]) };
    }
    let index = match face {
        FontFace::UiSmall => 0,
        FontFace::UiText => 1,
        FontFace::UiDisplay => 2,
        FontFace::Icons => 3,
    };
    if let Some(name) = RESOLVED.with(|r| r.borrow()[index]) {
        return name;
    }
    let candidates = family(face);
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: `collection` is a valid out pointer for the duration of the call.
    let loaded = unsafe { dwrite.GetSystemFontCollection(&mut collection, false) };
    let found = match (loaded, collection) {
        (Ok(()), Some(collection)) => candidates.iter().copied().find(|name| {
            let (mut index, mut exists) = (0u32, BOOL(0));
            // SAFETY: the name is a valid NUL-terminated wide string for the call; the out
            // pointers are valid locals.
            let ok = unsafe {
                collection.FindFamilyName(&HSTRING::from(*name), &mut index, &mut exists)
            };
            ok.is_ok() && exists.as_bool()
        }),
        (Err(e), _) => {
            log::warn!("system font collection unavailable: {e}");
            None
        }
        (Ok(()), None) => None,
    };
    let name = found.unwrap_or(candidates[candidates.len() - 1]);
    if found.is_none() {
        log::warn!("no font of {candidates:?} resolves; using {name}");
    }
    RESOLVED.with(|r| r.borrow_mut()[index] = Some(name));
    name
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
        // SAFETY: plain factory constructors without pointer arguments.
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        // SAFETY: as above.
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        // SAFETY: CLSID_WICImagingFactory is a valid class id; COM is initialized by the caller.
        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        Ok(RenderFactory { d2d, dwrite, wic })
    }
}

/// A point for Direct2D.
fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// A straight ARGB colour as Direct2D's straight float colour (the render target premultiplies).
fn color_f(c: Argb) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(c.r()) / 255.0,
        g: f32::from(c.g()) / 255.0,
        b: f32::from(c.b()) / 255.0,
        a: f32::from(c.a()) / 255.0,
    }
}

fn rect_f(r: RectF) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

fn dwrite_weight(weight: FontWeight) -> DWRITE_FONT_WEIGHT {
    match weight {
        FontWeight::Regular => DWRITE_FONT_WEIGHT_NORMAL,
        FontWeight::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
    }
}

/// Large enough for any single line Lastdrop lays out without a box.
const UNBOUNDED: f32 = 100_000.0;

/// Text formats by font, created on first use (one thread).
struct Formats {
    dwrite: IDWriteFactory,
    cache: RefCell<Vec<(FontFace, u32, FontWeight, IDWriteTextFormat)>>,
    tabular: RefCell<Option<IDWriteTypography>>,
}

impl Formats {
    fn new(dwrite: IDWriteFactory) -> Formats {
        Formats {
            dwrite,
            cache: RefCell::new(Vec::new()),
            tabular: RefCell::new(None),
        }
    }

    /// OpenType `tnum`: Segoe UI Variable's default figures are proportional in DirectWrite
    /// (`0000000000` is 66.3 px at 12 px, `1111111111` 47.5), so R§5.6's "digits are tabular"
    /// needs the feature, or numbers jitter as they change.
    fn tabular(&self) -> windows::core::Result<IDWriteTypography> {
        if let Some(t) = self.tabular.borrow().as_ref() {
            return Ok(t.clone());
        }
        // SAFETY: plain constructor and setter on live objects.
        let typography = unsafe {
            let t = self.dwrite.CreateTypography()?;
            for tag in [
                DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
                DWRITE_FONT_FEATURE_TAG_KERNING,
                DWRITE_FONT_FEATURE_TAG_STANDARD_LIGATURES,
                DWRITE_FONT_FEATURE_TAG_CONTEXTUAL_ALTERNATES,
                DWRITE_FONT_FEATURE_TAG_CONTEXTUAL_LIGATURES,
            ] {
                t.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: tag,
                    parameter: 1,
                })?;
            }
            t
        };
        *self.tabular.borrow_mut() = Some(typography.clone());
        Ok(typography)
    }

    fn format(&self, font: &Font) -> windows::core::Result<IDWriteTextFormat> {
        let size_bits = font.size_px.to_bits();
        if let Some((.., f)) = self.cache.borrow().iter().find(|(face, size, weight, _)| {
            *face == font.face && *size == size_bits && *weight == font.weight
        }) {
            return Ok(f.clone());
        }
        let name = resolve_family(&self.dwrite, font.face);
        // SAFETY: every pointer argument is a valid wide string for the call; `None` selects the
        // system font collection.
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                &HSTRING::from(name),
                None,
                dwrite_weight(font.weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font.size_px.max(0.1),
                w!("en-us"),
            )?
        };
        // SAFETY: plain setter on a live format.
        unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };
        let mut cache = self.cache.borrow_mut();
        // A readout uses a handful of fonts; keep the cache from growing without bound if DPI
        // changes many times.
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.push((font.face, size_bits, font.weight, format.clone()));
        Ok(format)
    }

    /// A one-line layout of `text` in a `max_w × max_h` box, with `letter_spacing` after every
    /// character (`IDWriteTextLayout1::SetCharacterSpacing`).
    fn layout(
        &self,
        text: &str,
        font: &Font,
        max_w: f32,
        max_h: f32,
        letter_spacing: f32,
    ) -> windows::core::Result<IDWriteTextLayout> {
        let format = self.format(font)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `wide` and `format` outlive the call.
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, &format, max_w.max(0.0), max_h.max(0.0))?
        };
        let range = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: u32::try_from(wide.len()).unwrap_or(u32::MAX),
        };
        if font.face != FontFace::Icons {
            let tabular = self.tabular()?;
            // SAFETY: plain setter on a live layout.
            unsafe { layout.SetTypography(&tabular, range)? };
        }
        if letter_spacing != 0.0 {
            let layout1: IDWriteTextLayout1 = layout.cast()?;
            // SAFETY: plain setter on a live layout.
            unsafe { layout1.SetCharacterSpacing(0.0, letter_spacing, 0.0, range)? };
        }
        Ok(layout)
    }

    /// The advance width of `text` (trailing spacing included), device px.
    fn advance(&self, text: &str, font: &Font, letter_spacing: f32) -> windows::core::Result<f32> {
        if text.is_empty() {
            return Ok(0.0);
        }
        let layout = self.layout(text, font, UNBOUNDED, UNBOUNDED, letter_spacing)?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        // SAFETY: `metrics` is a valid out pointer.
        unsafe { layout.GetMetrics(&mut metrics)? };
        Ok(metrics.widthIncludingTrailingWhitespace)
    }
}

/// The baseline of the first line of `layout`, from the layout's top.
fn first_baseline(layout: &IDWriteTextLayout) -> windows::core::Result<f32> {
    let mut lines = [DWRITE_LINE_METRICS::default(); 1];
    let mut count = 0u32;
    // SAFETY: the slice and the count are valid for the call. A layout with more than one line
    // reports E_NOT_SUFFICIENT_BUFFER; the first line is filled either way.
    let result = unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count) };
    if count == 0 {
        result?;
    }
    Ok(lines[0].baseline)
}

/// A premultiplied-BGRA bitmap with a software Direct2D render target.
pub struct Canvas {
    bitmap: IWICBitmap,
    target: ID2D1RenderTarget,
    width: u32,
    height: u32,
    d2d: ID2D1Factory,
    formats: Formats,
    brush: ID2D1SolidColorBrush,
    /// Stroke styles by (round caps, round joins).
    strokes: [Option<ID2D1StrokeStyle>; 4],
    drawing: bool,
    /// The first error of the current frame; [`Canvas::end`] returns it.
    error: Option<windows::core::Error>,
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
        let (width, height) = (width.max(1), height.max(1));
        // SAFETY: the pixel format GUID is a valid static for the call.
        let bitmap = unsafe {
            factory.wic.CreateBitmap(
                width,
                height,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )?
        };
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            // 96 DPI: one DIP is one device pixel; callers work in device px.
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        // SAFETY: `bitmap` and `props` are valid for the call.
        let target = unsafe { factory.d2d.CreateWicBitmapRenderTarget(&bitmap, &props)? };
        // SAFETY: plain setters on a live render target; the brush colour pointer is a local.
        let brush = unsafe {
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            target.CreateSolidColorBrush(&color_f(Argb::TRANSPARENT), None)?
        };
        Ok(Canvas {
            bitmap,
            target,
            width,
            height,
            d2d: factory.d2d.clone(),
            formats: Formats::new(factory.dwrite.clone()),
            brush,
            strokes: [None, None, None, None],
            drawing: false,
            error: None,
        })
    }

    /// Width in device px.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in device px.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Records the first error of the frame.
    fn note<T>(&mut self, result: windows::core::Result<T>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e);
                }
                None
            }
        }
    }

    /// The shared brush, set to `color`.
    fn brush(&self, color: Argb) -> &ID2D1SolidColorBrush {
        // SAFETY: the colour pointer is a local valid for the call.
        unsafe { self.brush.SetColor(&color_f(color)) };
        &self.brush
    }

    /// Starts a frame, clearing to `clear` (the readout clears to the hit-test colour
    /// `0x01000000`, R§6.1).
    pub fn begin(&mut self, clear: Argb) {
        if self.drawing {
            return;
        }
        self.error = None;
        self.drawing = true;
        // SAFETY: BeginDraw/Clear on a live render target; the colour is a local.
        unsafe {
            self.target.BeginDraw();
            self.target.Clear(Some(&color_f(clear)));
        }
    }

    /// Ends the frame.
    pub fn end(&mut self) -> windows::core::Result<()> {
        if !self.drawing {
            return Ok(());
        }
        self.drawing = false;
        // SAFETY: matches the BeginDraw of `begin`.
        let ended = unsafe { self.target.EndDraw(None, None) };
        match self.error.take() {
            Some(e) => Err(e),
            None => ended,
        }
    }

    /// A filled axis-aligned rectangle (bar segments, ticks: exact device pixels, aliased).
    pub fn fill_rect(&mut self, rect: RectF, color: Argb) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let brush = self.brush(color);
        // SAFETY: the rect is a local; the brush belongs to this render target.
        unsafe {
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            self.target.FillRectangle(&rect_f(rect), brush);
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
    }

    /// A filled rounded rectangle (hover plate radius 4, tray tiles, rounded fill ends).
    pub fn fill_rounded_rect(&mut self, rect: RectF, radius: f32, color: Argb) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let rounded = D2D1_ROUNDED_RECT {
            rect: rect_f(rect),
            radiusX: radius.max(0.0),
            radiusY: radius.max(0.0),
        };
        let brush = self.brush(color);
        // SAFETY: the rect is a local; the brush belongs to this render target.
        unsafe { self.target.FillRoundedRectangle(&rounded, brush) };
    }

    /// A stroked rounded rectangle, the stroke centred on `rect` (outlined tray tile, high
    /// contrast plate outline, monogram box).
    pub fn stroke_rounded_rect(&mut self, rect: RectF, radius: f32, width: f32, color: Argb) {
        if rect.w <= 0.0 || rect.h <= 0.0 || width <= 0.0 {
            return;
        }
        let rounded = D2D1_ROUNDED_RECT {
            rect: rect_f(rect),
            radiusX: radius.max(0.0),
            radiusY: radius.max(0.0),
        };
        let brush = self.brush(color);
        // SAFETY: the rect is a local; the brush belongs to this render target; no stroke style.
        unsafe {
            self.target
                .DrawRoundedRectangle(&rounded, brush, width, None)
        };
    }

    /// A line.
    pub fn line(&mut self, from: PointF, to: PointF, width: f32, color: Argb) {
        let brush = self.brush(color);
        // SAFETY: plain values; the brush belongs to this render target.
        unsafe {
            self.target
                .DrawLine(v2(from.x, from.y), v2(to.x, to.y), brush, width, None)
        };
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
        if text.is_empty() {
            return;
        }
        let (layout, origin, options) = match placement {
            TextPlacement::LineBox { rect, align } => {
                let layout = self
                    .formats
                    .layout(text, font, rect.w, rect.h, letter_spacing);
                let Some(layout) = self.note(layout) else {
                    return;
                };
                let alignment = match align {
                    Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                    Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                };
                if self.note(set_alignment(&layout, alignment)).is_none() {
                    return;
                }
                (layout, v2(rect.x, rect.y), D2D1_DRAW_TEXT_OPTIONS_NONE)
            }
            TextPlacement::Baseline { origin } => {
                let layout = self
                    .formats
                    .layout(text, font, UNBOUNDED, UNBOUNDED, letter_spacing);
                let Some(layout) = self.note(layout) else {
                    return;
                };
                let Some(baseline) = self.note(first_baseline(&layout)) else {
                    return;
                };
                (
                    layout,
                    v2(origin.x, origin.y - baseline),
                    D2D1_DRAW_TEXT_OPTIONS_NO_SNAP,
                )
            }
        };
        self.draw_layout(&layout, origin, options, color);
    }

    fn draw_layout(
        &mut self,
        layout: &IDWriteTextLayout,
        origin: Vector2,
        options: D2D1_DRAW_TEXT_OPTIONS,
        color: Argb,
    ) {
        let brush = self.brush(color);
        // SAFETY: the layout and brush are live COM objects for the call.
        unsafe { self.target.DrawTextLayout(origin, layout, brush, options) };
    }

    /// One Segoe Fluent Icons glyph at `size_px`, centred in `cell` (R§6.5 glyph cells; severity
    /// glyphs draw their two layers with two calls).
    pub fn glyph(&mut self, ch: char, size_px: f32, cell: RectF, color: Argb) {
        let font = Font {
            face: FontFace::Icons,
            size_px,
            weight: FontWeight::Regular,
        };
        let mut buf = [0u8; 4];
        let text: &str = ch.encode_utf8(&mut buf);
        let layout = self.formats.layout(text, &font, cell.w, cell.h, 0.0);
        let Some(layout) = self.note(layout) else {
            return;
        };
        if self
            .note(set_alignment(&layout, DWRITE_TEXT_ALIGNMENT_CENTER))
            .is_none()
        {
            return;
        }
        self.draw_layout(
            &layout,
            v2(cell.x, cell.y),
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            color,
        );
    }

    /// An SVG path (provider glyphs, the drop) scaled by `scale` and moved to `origin`.
    pub fn path(&mut self, svg_d: &str, origin: PointF, scale: f32, paint: PathPaint) {
        let segments = match svg::parse(svg_d) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("bad path data at byte {}: {}", e.at, e.what);
                return;
            }
        };
        let map = |x: f32, y: f32| v2(origin.x + x * scale, origin.y + y * scale);
        let geometry = self.note(build_geometry(&self.d2d, &segments, map, scale));
        let Some(geometry) = geometry else {
            return;
        };
        self.paint_geometry(&geometry, paint);
    }

    fn paint_geometry(&mut self, geometry: &ID2D1PathGeometry, paint: PathPaint) {
        match paint {
            PathPaint::Fill(color) => {
                let brush = self.brush(color);
                // SAFETY: live COM objects for the call.
                unsafe { self.target.FillGeometry(geometry, brush, None) };
            }
            PathPaint::Stroke {
                color,
                width,
                round_caps,
                round_joins,
            } => {
                let style = self.stroke_style(round_caps, round_joins);
                let Some(style) = self.note(style) else {
                    return;
                };
                let brush = self.brush(color);
                // SAFETY: live COM objects for the call.
                unsafe { self.target.DrawGeometry(geometry, brush, width, &style) };
            }
        }
    }

    fn stroke_style(
        &mut self,
        round_caps: bool,
        round_joins: bool,
    ) -> windows::core::Result<ID2D1StrokeStyle> {
        let slot = usize::from(round_caps) * 2 + usize::from(round_joins);
        if let Some(style) = &self.strokes[slot] {
            return Ok(style.clone());
        }
        let cap = if round_caps {
            D2D1_CAP_STYLE_ROUND
        } else {
            D2D1_CAP_STYLE_FLAT
        };
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: cap,
            endCap: cap,
            dashCap: cap,
            lineJoin: if round_joins {
                D2D1_LINE_JOIN_ROUND
            } else {
                D2D1_LINE_JOIN_MITER
            },
            // The canvas default, as in the reference drawing code.
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        };
        // SAFETY: `props` is a local valid for the call; no dashes.
        let style = unsafe { self.d2d.CreateStrokeStyle(&props, None)? };
        self.strokes[slot] = Some(style.clone());
        Ok(style)
    }

    /// A fill segment with optional semicircular ends, as the reference drawing code's `pill`:
    /// `r = min(h/2, w/2)`, arcs centred on the segment's middle line.
    fn pill(&mut self, rect: RectF, round_left: bool, round_right: bool, color: Argb) {
        let RectF { x, y, w, h } = rect;
        let r = (h / 2.0).min(w / 2.0);
        let mid = y + h / 2.0;
        let left = if round_left { x + r } else { x };
        let right = if round_right { x + w - r } else { x + w };
        let mut segs = vec![svg::Segment::Move(left, y), svg::Segment::Line(right, y)];
        if round_right {
            segs.push(svg::Segment::Line(x + w - r, mid - r));
            segs.push(svg::Segment::Arc {
                rx: r,
                ry: r,
                rotation: 0.0,
                large: false,
                sweep: true,
                x: x + w - r,
                y: mid + r,
            });
        } else {
            segs.push(svg::Segment::Line(x + w, y));
            segs.push(svg::Segment::Line(x + w, y + h));
        }
        segs.push(svg::Segment::Line(left, y + h));
        if round_left {
            segs.push(svg::Segment::Line(x + r, mid + r));
            segs.push(svg::Segment::Arc {
                rx: r,
                ry: r,
                rotation: 0.0,
                large: false,
                sweep: true,
                x: x + r,
                y: mid - r,
            });
        } else {
            segs.push(svg::Segment::Line(x, y + h));
            segs.push(svg::Segment::Line(x, y));
        }
        segs.push(svg::Segment::Close);
        let geometry = self.note(build_geometry(&self.d2d, &segs, v2, 1.0));
        if let Some(geometry) = geometry {
            self.paint_geometry(&geometry, PathPaint::Fill(color));
        }
    }

    /// A pace bar at `origin` (the bar's row box top-left) with the geometry of
    /// `lastdrop_core::readout::bar_geometry`: track and tick aliased, fill segments with rounded
    /// outer ends where `round_start`/`round_end` say (radius `fill_h / 2`), dashes square.
    pub fn bar(&mut self, geometry: &BarGeometry, origin: PointF, colors: BarColors) {
        let g = geometry;
        let x = |px: i32| origin.x + px as f32;
        let y = |px: i32| origin.y + px as f32;
        for s in &g.track {
            let rect = RectF {
                x: x(s.x0),
                y: y(g.y_track),
                w: (s.x1 - s.x0) as f32,
                h: g.hair as f32,
            };
            self.fill_rect(rect, colors.track);
        }
        let last = g.fill.len().saturating_sub(1);
        for (i, s) in g.fill.iter().enumerate() {
            let rect = RectF {
                x: x(s.x0),
                y: y(g.y_fill),
                w: (s.x1 - s.x0) as f32,
                h: g.fill_h as f32,
            };
            let round_left = g.round_start && i == 0 && s.x0 == 0;
            let round_right = g.round_end && i == last && s.x1 == g.fill_end;
            if round_left || round_right {
                self.pill(rect, round_left, round_right, colors.fill);
            } else {
                self.fill_rect(rect, colors.fill);
            }
        }
        if let Some(k) = g.tick {
            let rect = RectF {
                x: x(k),
                y: y(g.y_tick),
                w: g.hair as f32,
                h: g.tick_h as f32,
            };
            self.fill_rect(rect, colors.tick);
        }
    }

    /// The pixels, premultiplied BGRA, top-down, stride `width × 4`.
    pub fn pixels(&self) -> windows::core::Result<Vec<u8>> {
        let stride = self.width * 4;
        let len = usize::try_from(u64::from(stride) * u64::from(self.height))
            .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY))?;
        let mut buf = vec![0u8; len];
        // SAFETY: a null rect copies the whole bitmap; `buf` holds `stride × height` bytes.
        unsafe { self.bitmap.CopyPixels(std::ptr::null(), stride, &mut buf)? };
        Ok(buf)
    }
}

fn set_alignment(
    layout: &IDWriteTextLayout,
    alignment: DWRITE_TEXT_ALIGNMENT,
) -> windows::core::Result<()> {
    // SAFETY: plain setters on a live layout.
    unsafe {
        layout.SetTextAlignment(alignment)?;
        layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
    }
}

/// Builds a path geometry from parsed segments, mapping every point with `map` and every radius
/// with `radius_scale`.
fn build_geometry(
    d2d: &ID2D1Factory,
    segments: &[svg::Segment],
    map: impl Fn(f32, f32) -> Vector2,
    radius_scale: f32,
) -> windows::core::Result<ID2D1PathGeometry> {
    // SAFETY: plain constructor.
    let geometry = unsafe { d2d.CreatePathGeometry()? };
    // SAFETY: the geometry is live and not yet opened.
    let sink: ID2D1GeometrySink = unsafe { geometry.Open()? };
    // SAFETY: plain setter on an open sink.
    unsafe { sink.SetFillMode(D2D1_FILL_MODE_WINDING) };
    let mut open = false;
    let mut current = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    let begin = |sink: &ID2D1GeometrySink, p: (f32, f32), open: &mut bool| {
        if !*open {
            // SAFETY: the sink is open; a figure begins only when none is open.
            unsafe { sink.BeginFigure(map(p.0, p.1), D2D1_FIGURE_BEGIN_FILLED) };
            *open = true;
        }
    };
    for seg in segments {
        match *seg {
            svg::Segment::Move(x, y) => {
                if open {
                    // SAFETY: a figure is open.
                    unsafe { sink.EndFigure(D2D1_FIGURE_END_OPEN) };
                    open = false;
                }
                current = (x, y);
                start = (x, y);
                begin(&sink, current, &mut open);
            }
            svg::Segment::Line(x, y) => {
                begin(&sink, current, &mut open);
                // SAFETY: a figure is open.
                unsafe { sink.AddLine(map(x, y)) };
                current = (x, y);
            }
            svg::Segment::Cubic(x1, y1, x2, y2, x, y) => {
                begin(&sink, current, &mut open);
                let bezier = D2D1_BEZIER_SEGMENT {
                    point1: map(x1, y1),
                    point2: map(x2, y2),
                    point3: map(x, y),
                };
                // SAFETY: a figure is open; the segment is a local.
                unsafe { sink.AddBezier(&bezier) };
                current = (x, y);
            }
            svg::Segment::Arc {
                rx,
                ry,
                rotation,
                large,
                sweep,
                x,
                y,
            } => {
                begin(&sink, current, &mut open);
                let arc = D2D1_ARC_SEGMENT {
                    point: map(x, y),
                    size: D2D_SIZE_F {
                        width: rx.abs() * radius_scale,
                        height: ry.abs() * radius_scale,
                    },
                    rotationAngle: rotation,
                    // SVG's sweep flag 1 is the positive-angle direction, which is clockwise on a
                    // y-down screen, as Direct2D's CLOCKWISE.
                    sweepDirection: if sweep {
                        D2D1_SWEEP_DIRECTION_CLOCKWISE
                    } else {
                        D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                    },
                    arcSize: if large {
                        D2D1_ARC_SIZE_LARGE
                    } else {
                        D2D1_ARC_SIZE_SMALL
                    },
                };
                // SAFETY: a figure is open; the segment is a local.
                unsafe { sink.AddArc(&arc) };
                current = (x, y);
            }
            svg::Segment::Close => {
                if open {
                    // SAFETY: a figure is open.
                    unsafe { sink.EndFigure(D2D1_FIGURE_END_CLOSED) };
                    open = false;
                }
                current = start;
            }
        }
    }
    if open {
        // SAFETY: a figure is open.
        unsafe { sink.EndFigure(D2D1_FIGURE_END_OPEN) };
    }
    // SAFETY: closes the sink opened above.
    unsafe { sink.Close()? };
    Ok(geometry)
}

/// Measures text with DirectWrite for `lastdrop_core::readout::layout::compute_layout` (R§6.3).
pub struct TextMeasurer {
    formats: Formats,
}

impl TextMeasurer {
    /// A measurer over the thread's DirectWrite factory.
    pub fn new(factory: &RenderFactory) -> TextMeasurer {
        TextMeasurer {
            formats: Formats::new(factory.dwrite.clone()),
        }
    }

    /// The advance of `text` in `font` with `letter_spacing` after every character (the tray
    /// digits, R§9.2: `w = Σ advances + n·ls`), device px.
    pub fn advance_font(&self, text: &str, font: &Font, letter_spacing: f32) -> f32 {
        match self.formats.advance(text, font, letter_spacing) {
            Ok(w) => w,
            Err(e) => {
                // A rough stand-in so a layout still has room; never expected in practice.
                log::warn!("text measurement failed: {e}");
                text.chars().count() as f32 * font.size_px * 0.6
            }
        }
    }
}

impl TextMeasure for TextMeasurer {
    fn advance(&self, text: &str, style: TextStyle, scale: f64) -> f32 {
        self.advance_font(text, &Font::scaled(style, scale), 0.0)
    }
}

/// A small SVG path-data parser: `M L H V C S A Z`, absolute and relative, implicit repeats
/// (R Appendix A, R§9.2 drop). Output is absolute.
mod svg {
    /// One absolute path segment.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(super) enum Segment {
        Move(f32, f32),
        Line(f32, f32),
        Cubic(f32, f32, f32, f32, f32, f32),
        Arc {
            rx: f32,
            ry: f32,
            rotation: f32,
            large: bool,
            sweep: bool,
            x: f32,
            y: f32,
        },
        Close,
    }

    /// Where and why parsing failed.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct ParseError {
        pub at: usize,
        pub what: &'static str,
    }

    struct Cursor<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl Cursor<'_> {
        fn skip_separators(&mut self) {
            while self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_ascii_whitespace() || *c == b',')
            {
                self.i += 1;
            }
        }

        fn error(&self, what: &'static str) -> ParseError {
            ParseError { at: self.i, what }
        }

        fn number(&mut self) -> Result<f32, ParseError> {
            self.skip_separators();
            let start = self.i;
            let s = self.s;
            if matches!(s.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            let mut digits = false;
            while s.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
                digits = true;
            }
            if s.get(self.i) == Some(&b'.') {
                self.i += 1;
                while s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                    digits = true;
                }
            }
            if !digits {
                self.i = start;
                return Err(self.error("expected a number"));
            }
            if matches!(s.get(self.i), Some(b'e' | b'E')) {
                let mark = self.i;
                self.i += 1;
                if matches!(s.get(self.i), Some(b'-' | b'+')) {
                    self.i += 1;
                }
                if s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    while s.get(self.i).is_some_and(u8::is_ascii_digit) {
                        self.i += 1;
                    }
                } else {
                    self.i = mark;
                }
            }
            std::str::from_utf8(&s[start..self.i])
                .ok()
                .and_then(|t| t.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .ok_or(ParseError {
                    at: start,
                    what: "bad number",
                })
        }

        /// An arc flag: a single `0` or `1`, separators optional.
        fn flag(&mut self) -> Result<bool, ParseError> {
            self.skip_separators();
            match self.s.get(self.i) {
                Some(b'0') => {
                    self.i += 1;
                    Ok(false)
                }
                Some(b'1') => {
                    self.i += 1;
                    Ok(true)
                }
                _ => Err(self.error("expected an arc flag")),
            }
        }
    }

    /// Parses path data into absolute segments.
    pub(super) fn parse(d: &str) -> Result<Vec<Segment>, ParseError> {
        let mut c = Cursor {
            s: d.as_bytes(),
            i: 0,
        };
        let mut out = Vec::new();
        let (mut cx, mut cy) = (0.0f32, 0.0f32);
        let (mut sx, mut sy) = (0.0f32, 0.0f32);
        // The second control point of the previous cubic, for `S`.
        let mut last_ctrl: Option<(f32, f32)> = None;
        let mut command: Option<u8> = None;
        loop {
            c.skip_separators();
            let Some(&ch) = c.s.get(c.i) else {
                break;
            };
            let cmd = if ch.is_ascii_alphabetic() {
                c.i += 1;
                ch
            } else {
                // Implicit repetition; after a moveto the pairs are linetos.
                match command {
                    Some(b'M') => b'L',
                    Some(b'm') => b'l',
                    Some(b'Z' | b'z') | None => return Err(c.error("expected a command")),
                    Some(other) => other,
                }
            };
            command = Some(cmd);
            let rel = cmd.is_ascii_lowercase();
            let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
            let mut ctrl = None;
            match cmd.to_ascii_uppercase() {
                b'M' => {
                    let (x, y) = (ox + c.number()?, oy + c.number()?);
                    out.push(Segment::Move(x, y));
                    (cx, cy, sx, sy) = (x, y, x, y);
                }
                b'L' => {
                    let (x, y) = (ox + c.number()?, oy + c.number()?);
                    out.push(Segment::Line(x, y));
                    (cx, cy) = (x, y);
                }
                b'H' => {
                    let x = ox + c.number()?;
                    out.push(Segment::Line(x, cy));
                    cx = x;
                }
                b'V' => {
                    let y = oy + c.number()?;
                    out.push(Segment::Line(cx, y));
                    cy = y;
                }
                b'C' => {
                    let (x1, y1) = (ox + c.number()?, oy + c.number()?);
                    let (x2, y2) = (ox + c.number()?, oy + c.number()?);
                    let (x, y) = (ox + c.number()?, oy + c.number()?);
                    out.push(Segment::Cubic(x1, y1, x2, y2, x, y));
                    ctrl = Some((x2, y2));
                    (cx, cy) = (x, y);
                }
                b'S' => {
                    let (x1, y1) = match last_ctrl {
                        Some((px, py)) => (2.0 * cx - px, 2.0 * cy - py),
                        None => (cx, cy),
                    };
                    let (x2, y2) = (ox + c.number()?, oy + c.number()?);
                    let (x, y) = (ox + c.number()?, oy + c.number()?);
                    out.push(Segment::Cubic(x1, y1, x2, y2, x, y));
                    ctrl = Some((x2, y2));
                    (cx, cy) = (x, y);
                }
                b'A' => {
                    let rx = c.number()?;
                    let ry = c.number()?;
                    let rotation = c.number()?;
                    let large = c.flag()?;
                    let sweep = c.flag()?;
                    let (x, y) = (ox + c.number()?, oy + c.number()?);
                    out.push(Segment::Arc {
                        rx,
                        ry,
                        rotation,
                        large,
                        sweep,
                        x,
                        y,
                    });
                    (cx, cy) = (x, y);
                }
                b'Z' => {
                    out.push(Segment::Close);
                    (cx, cy) = (sx, sy);
                }
                _ => return Err(c.error("unsupported command")),
            }
            last_ctrl = ctrl;
        }
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::{Segment as S, parse};

        #[test]
        fn parses_the_appendix_paths_and_the_drop() {
            let claude = parse("M6 1.15v9.7M1.8 3.58l8.4 4.84M1.8 8.42l8.4-4.84").unwrap();
            assert_eq!(claude.len(), 6);
            assert_eq!(claude[1], S::Line(6.0, 1.15 + 9.7));
            assert_eq!(claude[3], S::Line(1.8 + 8.4, 3.58 + 4.84));
            assert_eq!(claude[5], S::Line(1.8 + 8.4, 8.42 - 4.84));

            let codex = parse("M3.35 4.35 5.15 6 3.35 7.65M6.35 8.05h2.4").unwrap();
            assert_eq!(
                codex,
                vec![
                    S::Move(3.35, 4.35),
                    S::Line(5.15, 6.0),
                    S::Line(3.35, 7.65),
                    S::Move(6.35, 8.05),
                    S::Line(6.35 + 2.4, 8.05),
                ]
            );

            let drop = parse(lastdrop_core::glyph::DROP_PATH).unwrap();
            assert_eq!(drop.len(), 5);
            assert_eq!(
                drop[2],
                S::Arc {
                    rx: 4.8,
                    ry: 4.8,
                    rotation: 0.0,
                    large: false,
                    sweep: false,
                    x: 12.8,
                    y: 10.1
                }
            );
            assert_eq!(drop[4], S::Close);

            let meshy = parse("M6 1.3 11.1 10.4H0.9z").unwrap();
            assert_eq!(meshy.last(), Some(&S::Close));
            assert_eq!(meshy[2], S::Line(0.9, 10.4));
        }

        #[test]
        fn handles_compact_numbers_flags_and_errors() {
            let p = parse("M0,0L.5.5l-1e1-2").unwrap();
            assert_eq!(p[1], S::Line(0.5, 0.5));
            assert_eq!(p[2], S::Line(0.5 - 10.0, 0.5 - 2.0));
            let a = parse("M0 0a1 1 0 011 1").unwrap();
            assert!(matches!(
                a[1],
                S::Arc {
                    large: false,
                    sweep: true,
                    x: 1.0,
                    y: 1.0,
                    ..
                }
            ));
            assert!(parse("M0 0Q1 1 2 2").is_err());
            assert!(parse("1 2").is_err());
            assert!(parse("M0").is_err());
        }

        #[test]
        fn smooth_cubics_reflect_the_previous_control_point() {
            let p = parse("M0 0C1 0 2 1 2 2S3 4 4 4").unwrap();
            assert_eq!(p[2], S::Cubic(2.0, 3.0, 3.0, 4.0, 4.0, 4.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use lastdrop_core::color::ColorRole;
    use lastdrop_core::readout::bar::{BarInput, BarMode, bar_geometry};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    use super::*;

    fn factory() -> RenderFactory {
        // SAFETY: initializes COM on the test thread (S_FALSE when already initialized is fine).
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        RenderFactory::new().expect("factories")
    }

    fn px(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn families_resolve_to_a_candidate() {
        let f = factory();
        for face in [
            FontFace::UiSmall,
            FontFace::UiText,
            FontFace::UiDisplay,
            FontFace::Icons,
        ] {
            let name = resolve_family(&f.dwrite, face);
            assert!(family(face).contains(&name), "{face:?} → {name}");
        }
    }

    #[test]
    fn clears_to_the_hit_test_colour_and_fills_exact_pixels() {
        let f = factory();
        let mut c = Canvas::new(&f, 8, 4).unwrap();
        c.begin(Argb(0x0100_0000));
        c.fill_rect(
            RectF {
                x: 2.0,
                y: 1.0,
                w: 3.0,
                h: 2.0,
            },
            Argb(0xFFFF_0000),
        );
        c.end().unwrap();
        let p = c.pixels().unwrap();
        assert_eq!(p.len(), 8 * 4 * 4);
        assert_eq!(px(&p, 8, 0, 0), [0, 0, 0, 1]);
        assert_eq!(px(&p, 8, 2, 1), [0, 0, 255, 255]);
        assert_eq!(px(&p, 8, 4, 2), [0, 0, 255, 255]);
        assert_eq!(px(&p, 8, 5, 2), [0, 0, 0, 1]);
        assert_eq!(px(&p, 8, 2, 3), [0, 0, 0, 1]);
    }

    #[test]
    fn bars_paint_the_r13_rectangles_and_leave_the_gaps_empty() {
        // R§13: s = 1, u .42, e .5533: fill [0,17), track [17,21) [22,23) [24,40), tick at 22.
        let f = factory();
        let input = BarInput {
            used: 0.42,
            elapsed: Some(0.5533),
            mode: BarMode::Normal,
            dashed: false,
            fill: ColorRole::TextPrimary,
            tick: ColorRole::TextPrimary,
            track: ColorRole::Track,
        };
        let g = bar_geometry(&input, 40.0, 16.0, 1.0);
        let mut c = Canvas::new(&f, 40, 16).unwrap();
        c.begin(Argb::TRANSPARENT);
        let colors = BarColors {
            fill: Argb(0xFFFF_FFFF),
            track: Argb(0xFF00_00FF),
            tick: Argb(0xFF00_FF00),
        };
        c.bar(&g, PointF { x: 0.0, y: 0.0 }, colors);
        c.end().unwrap();
        let p = c.pixels().unwrap();
        // Fill row 7 (yFill 6, height 3), inside the fill: white.
        assert_eq!(px(&p, 40, 8, 7), [255, 255, 255, 255]);
        // Track row 7 (yTrack 7, height 1): blue at 18, nothing in the gaps 21 and 23.
        assert_eq!(px(&p, 40, 18, 7), [255, 0, 0, 255]);
        assert_eq!(px(&p, 40, 21, 7), [0, 0, 0, 0]);
        assert_eq!(px(&p, 40, 23, 7), [0, 0, 0, 0]);
        // Tick column 22 from yTick 3 for 9 rows: green.
        assert_eq!(px(&p, 40, 22, 3), [0, 255, 0, 255]);
        assert_eq!(px(&p, 40, 22, 11), [0, 255, 0, 255]);
        assert_eq!(px(&p, 40, 22, 12), [0, 0, 0, 0]);
        // Rounded fill start: the corner pixel is only partly covered.
        let corner = px(&p, 40, 0, 6);
        assert!(corner[3] > 0 && corner[3] < 255, "{corner:?}");
        // Nothing above or below the bar.
        assert_eq!(px(&p, 40, 8, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn text_and_glyphs_draw_something_inside_their_boxes() {
        let f = factory();
        let mut c = Canvas::new(&f, 60, 16).unwrap();
        c.begin(Argb::TRANSPARENT);
        let font = Font::scaled(TextStyle::ROW, 1.0);
        c.text(
            "42%",
            &font,
            TextPlacement::LineBox {
                rect: RectF {
                    x: 0.0,
                    y: 0.0,
                    w: 24.0,
                    h: 16.0,
                },
                align: Align::Right,
            },
            0.0,
            Argb(0xFFFF_FFFF),
        );
        c.glyph(
            '\u{E777}',
            11.0,
            RectF {
                x: 30.0,
                y: 0.0,
                w: 11.0,
                h: 16.0,
            },
            Argb(0xFFFF_FFFF),
        );
        c.end().unwrap();
        let p = c.pixels().unwrap();
        let ink = |x0: u32, x1: u32| (x0..x1).any(|x| (0..16).any(|y| px(&p, 60, x, y)[3] > 0));
        assert!(ink(0, 24), "value text drawn");
        assert!(!ink(24, 29), "right-aligned text stays in its box");
        assert!(ink(30, 41), "glyph drawn");
        assert!(!ink(42, 60));
    }

    #[test]
    fn paths_fill_and_stroke() {
        let f = factory();
        let mut c = Canvas::new(&f, 16, 16).unwrap();
        c.begin(Argb::TRANSPARENT);
        c.path(
            lastdrop_core::glyph::DROP_PATH,
            PointF { x: 0.0, y: 0.0 },
            1.0,
            PathPaint::Fill(Argb(0xFFFF_FFFF)),
        );
        c.end().unwrap();
        let p = c.pixels().unwrap();
        // Inside the round bottom of the drop, and outside it in the top corners.
        assert_eq!(px(&p, 16, 8, 10)[3], 255);
        assert_eq!(px(&p, 16, 1, 1)[3], 0);
        assert_eq!(px(&p, 16, 14, 1)[3], 0);

        c.begin(Argb::TRANSPARENT);
        c.path(
            "M2 8h12",
            PointF { x: 0.0, y: 0.0 },
            1.0,
            PathPaint::Stroke {
                color: Argb(0xFFFF_FFFF),
                width: 2.0,
                round_caps: false,
                round_joins: false,
            },
        );
        c.end().unwrap();
        let p = c.pixels().unwrap();
        assert_eq!(px(&p, 16, 8, 7)[3], 255);
        assert_eq!(px(&p, 16, 8, 9)[3], 0);
        assert_eq!(px(&p, 16, 0, 7)[3], 0);
    }

    #[test]
    fn measures_tabular_digits_and_letter_spacing() {
        let f = factory();
        let m = TextMeasurer::new(&f);
        let zeros = m.advance("0000000000", TextStyle::ROW, 1.0);
        let ones = m.advance("1111111111", TextStyle::ROW, 1.0);
        assert!(zeros > 0.0);
        assert!((zeros - ones).abs() < 0.01, "{zeros} vs {ones}");
        // Twice the size, twice the advance.
        let double = m.advance("42%", TextStyle::ROW, 2.0);
        let single = m.advance("42%", TextStyle::ROW, 1.0);
        assert!((double - 2.0 * single).abs() < 0.05);
        // w = Σ advances + n·ls.
        let font = Font {
            face: FontFace::UiSmall,
            size_px: 11.0,
            weight: FontWeight::Semibold,
        };
        let plain = m.advance_font("61", &font, 0.0);
        let spaced = m.advance_font("61", &font, -0.4);
        assert!((plain - 0.8 - spaced).abs() < 0.01, "{plain} {spaced}");
    }
}
