//! Draws one readout frame (R§6): the hit-test fill, the hover/pressed plate (R§6.8), and the
//! rows of the form — `[glyph 12] [4] [label LW] [6] [bar 40] [6] [right cell | compact cell]`
//! (R§6.2) — at the device geometry of a [`ReadoutLayout`] (R§6.1, R§6.3), with the pace bar of
//! `bar_geometry` (R§6.6) and the text runs of the model (R§5.7, R§5.8).
//!
//! Pure drawing: the model and layout decide everything; this module only converts logical
//! glyph cells and gaps to device px (`round(c × s)` on its own, R§0) and resolves colour roles.

use lastdrop_core::color::ColorRole;
use lastdrop_core::consts::layout::{
    BALANCE_APPROX_GAP, BAR_W, MONOGRAM_FONT, MONOGRAM_RADIUS, PLATE_RADIUS, ROW_H,
};
use lastdrop_core::glyph::{ProviderGlyph, provider_paths};
use lastdrop_core::readout::bar::{bar_geometry, segmented};
use lastdrop_core::readout::{
    FontFace, FontWeight, GlyphKind, ReadoutForm, ReadoutLayout, ReadoutModel, RowModel, Run,
    TextRun,
};

use crate::geom::{PointF, RectF};
use crate::palette::{Mode, mode, resolve};
use crate::render::{Align, BarColors, Canvas, Font, PathPaint, TextMeasurer, TextPlacement};
use crate::theme::ThemeInfo;

/// The hover or pressed plate (R§6.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Plate {
    /// No plate.
    None,
    /// Pointer over the slot: SubtleFillColorSecondary.
    Hover,
    /// Button down: SubtleFillColorTertiary.
    Pressed,
}

/// What one frame shows.
pub(super) struct FrameInput<'a> {
    pub model: &'a ReadoutModel,
    pub layout: &'a ReadoutLayout,
    pub form: ReadoutForm,
    pub theme: &'a ThemeInfo,
    pub plate: Plate,
    /// Floating above the taskbar (no app-button extents): the slot gets a card background so
    /// the text stays legible over whatever is behind it (Lastdrop's choice, not in the spec).
    pub floating: bool,
}

/// A logical constant in device px: `round(c × s)`, half up, on its own (R§0).
pub(super) fn px(c: f32, scale: f64) -> i32 {
    // Layout constants times a sane scale are small; `as` saturates otherwise.
    (f64::from(c) * scale + 0.5).floor() as i32
}

/// The slot size of `form`, device px (width × `slotH`).
pub(super) fn slot_size(layout: &ReadoutLayout, form: ReadoutForm) -> (i32, i32) {
    let w = match form {
        ReadoutForm::Compact => layout.width_compact,
        ReadoutForm::TwoRows | ReadoutForm::SingleRow | ReadoutForm::Hidden => layout.width_rows,
    };
    (w.max(1), layout.slot_h.max(1))
}

/// The rows drawn for `form`, with their row-box top (device px from the slot's top) and whether
/// the compact cell is used.
pub(super) fn rows_for<'m>(
    model: &'m ReadoutModel,
    layout: &ReadoutLayout,
    form: ReadoutForm,
) -> Vec<(&'m RowModel, i32, bool)> {
    match form {
        ReadoutForm::Hidden => Vec::new(),
        ReadoutForm::Compact => model
            .rows
            .get(model.compact_row)
            .or_else(|| model.rows.first())
            .map(|r| vec![(r, layout.y_single, true)])
            .unwrap_or_default(),
        ReadoutForm::SingleRow => model
            .rows
            .first()
            .map(|r| vec![(r, layout.y_single, false)])
            .unwrap_or_default(),
        ReadoutForm::TwoRows => match model.rows.as_slice() {
            [] => Vec::new(),
            [only] => vec![(only, layout.y_single, false)],
            [first, second, ..] => vec![
                (first, layout.y_row1, false),
                (second, layout.y_row2, false),
            ],
        },
    }
}

/// Draws the frame (begin to end). The canvas must be `slot_size(layout, form)`.
pub(super) fn draw(
    canvas: &mut Canvas,
    measurer: &TextMeasurer,
    input: &FrameInput<'_>,
) -> windows::core::Result<()> {
    let theme = input.theme;
    let s = input.layout.scale;
    let (w, h) = slot_size(input.layout, input.form);
    let slot = RectF {
        x: 0.0,
        y: 0.0,
        w: w as f32,
        h: h as f32,
    };
    let hair = (s.floor() as f32).max(1.0);
    let contrast = mode(theme) == Mode::HighContrast;
    canvas.begin(resolve(ColorRole::HitTest, theme));
    if input.floating {
        let radius = px(PLATE_RADIUS, s) as f32;
        canvas.fill_rounded_rect(slot, radius, resolve(ColorRole::CardBackground, theme));
        canvas.stroke_rounded_rect(
            inset(slot, hair / 2.0),
            radius,
            hair,
            resolve(ColorRole::CardBorder, theme),
        );
    }
    let plate_role = match input.plate {
        Plate::None => None,
        Plate::Hover => Some(ColorRole::SubtleSecondary),
        Plate::Pressed => Some(ColorRole::SubtleTertiary),
    };
    if let Some(role) = plate_role {
        let radius = px(PLATE_RADIUS, s) as f32;
        if contrast {
            // Contrast themes: a 1 px Highlight outline instead of a fill (R§6.8).
            canvas.stroke_rounded_rect(inset(slot, hair / 2.0), radius, hair, resolve(role, theme));
        } else {
            canvas.fill_rounded_rect(slot, radius, resolve(role, theme));
        }
    }
    for (row, y, compact) in rows_for(input.model, input.layout, input.form) {
        draw_row(canvas, measurer, row, input.layout, y, compact, theme);
    }
    canvas.end()
}

fn inset(r: RectF, by: f32) -> RectF {
    RectF {
        x: r.x + by,
        y: r.y + by,
        w: (r.w - 2.0 * by).max(0.0),
        h: (r.h - 2.0 * by).max(0.0),
    }
}

fn draw_row(
    canvas: &mut Canvas,
    measurer: &TextMeasurer,
    row: &RowModel,
    l: &ReadoutLayout,
    y: i32,
    compact: bool,
    theme: &ThemeInfo,
) {
    let s = l.scale;
    let row_h = l.row_h as f32;
    let yf = y as f32;
    if let (Some(x), Some(glyph)) = (l.x_glyph, &row.glyph) {
        draw_service_glyph(canvas, glyph, x, y, l.row_h, s, theme);
    }
    text_in_box(
        canvas,
        &row.label,
        RectF {
            x: l.x_label as f32,
            y: yf,
            w: l.label_w as f32,
            h: row_h,
        },
        Align::Left,
        s,
        theme,
    );
    let geometry = bar_geometry(&row.bar, BAR_W, ROW_H, s);
    // A theme's segmented look (themes-spec §2).
    let geometry = if theme.custom.segmented {
        segmented(geometry)
    } else {
        geometry
    };
    let colors = BarColors {
        fill: resolve(row.bar.fill, theme),
        track: resolve(row.bar.track, theme),
        tick: resolve(row.bar.tick, theme),
    };
    canvas.bar(
        &geometry,
        PointF {
            x: l.x_bar as f32,
            y: yf,
        },
        colors,
    );
    if compact {
        draw_runs(canvas, measurer, &row.compact.runs, l.x_right, y, l, theme);
        return;
    }
    if let Some(value) = &row.right.value {
        text_in_box(
            canvas,
            value,
            RectF {
                x: l.x_right as f32,
                y: yf,
                w: l.value_box_w as f32,
                h: row_h,
            },
            Align::Right,
            s,
            theme,
        );
    }
    draw_runs(
        canvas,
        measurer,
        &row.right.time,
        l.x_right + l.time_x,
        y,
        l,
        theme,
    );
    draw_runs(canvas, measurer, &row.right.say, l.x_right, y, l, theme);
}

fn text_in_box(
    canvas: &mut Canvas,
    run: &TextRun,
    rect: RectF,
    align: Align,
    scale: f64,
    theme: &ThemeInfo,
) {
    canvas.text(
        &run.text,
        &Font::scaled(run.style, scale),
        TextPlacement::LineBox { rect, align },
        0.0,
        resolve(run.role, theme),
    );
}

/// Runs left to right from `x0`: glyph cells (`round(cell s)` + `round(gap s)`) and text at its
/// measured advance; two text runs in a row (`≈` + duration) are `round(1 s)` apart (R§5.7).
fn draw_runs(
    canvas: &mut Canvas,
    measurer: &TextMeasurer,
    runs: &[Run],
    x0: i32,
    y: i32,
    l: &ReadoutLayout,
    theme: &ThemeInfo,
) {
    let s = l.scale;
    let row_h = l.row_h as f32;
    let mut x = x0 as f32;
    let mut after_text = false;
    for run in runs {
        match run {
            Run::Glyph(g) => {
                let cell_w = px(g.cell, s) as f32;
                let cell = RectF {
                    x,
                    y: y as f32,
                    w: cell_w,
                    h: row_h,
                };
                let size = (f64::from(g.size) * s) as f32;
                match g.glyph {
                    GlyphKind::Icon(icon) => {
                        canvas.glyph(icon.code_point(), size, cell, resolve(g.role, theme));
                    }
                    GlyphKind::Severity(severity) => {
                        for layer in severity.layers() {
                            canvas.glyph(layer.ch, size, cell, resolve(layer.role, theme));
                        }
                    }
                }
                x += cell_w + px(g.gap_after, s) as f32;
                after_text = false;
            }
            Run::Text(t) => {
                if after_text {
                    x += px(BALANCE_APPROX_GAP, s) as f32;
                }
                let font = Font::scaled(t.style, s);
                let advance = measurer.advance_font(&t.text, &font, 0.0);
                canvas.text(
                    &t.text,
                    &font,
                    TextPlacement::LineBox {
                        rect: RectF {
                            x,
                            y: y as f32,
                            w: advance.ceil() + 2.0,
                            h: row_h,
                        },
                        align: Align::Left,
                    },
                    0.0,
                    resolve(t.role, theme),
                );
                x += advance;
                after_text = true;
            }
        }
    }
}

/// The service glyph (R§6.4, R§6.7, R Appendix A) in a `round(12 s)` box, vertically centred in
/// the row: a drawn glyph's paths in TextPrimary (strokes scale with the box), or a monogram (1 px
/// rounded square r 2.5, the letter(s) Small semibold 8 px, or 7 px for two).
fn draw_service_glyph(
    canvas: &mut Canvas,
    glyph: &ProviderGlyph,
    x: i32,
    row_y: i32,
    row_h: i32,
    s: f64,
    theme: &ThemeInfo,
) {
    let box_px = px(12.0, s);
    let y = row_y + (row_h - box_px).div_euclid(2);
    let color = resolve(ColorRole::TextPrimary, theme);
    let origin = PointF {
        x: x as f32,
        y: y as f32,
    };
    match glyph {
        ProviderGlyph::Drawn(provider) => {
            let scale = box_px as f32 / 12.0;
            for path in provider_paths(*provider) {
                let paint = if path.fill {
                    PathPaint::Fill(color)
                } else {
                    PathPaint::Stroke {
                        color,
                        width: path.stroke_width * scale,
                        round_caps: path.round_caps,
                        round_joins: path.round_joins,
                    }
                };
                canvas.path(path.d, origin, scale, paint);
            }
        }
        ProviderGlyph::Monogram(letters) => {
            let hair = (s.floor() as f32).max(1.0);
            let b = RectF {
                x: origin.x,
                y: origin.y,
                w: box_px as f32,
                h: box_px as f32,
            };
            canvas.stroke_rounded_rect(
                inset(b, hair / 2.0),
                (f64::from(MONOGRAM_RADIUS) * s) as f32,
                hair,
                color,
            );
            let size = if letters.chars().count() > 1 {
                MONOGRAM_FONT.1
            } else {
                MONOGRAM_FONT.0
            };
            let font = Font {
                face: FontFace::UiSmall,
                size_px: (f64::from(size) * s) as f32,
                weight: FontWeight::Semibold,
            };
            canvas.text(
                letters,
                &font,
                TextPlacement::LineBox {
                    rect: b,
                    align: Align::Center,
                },
                0.0,
                color,
            );
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::BTreeMap;

    use lastdrop_core::grammar::meters::localize;
    use lastdrop_core::grammar::{Language, LocaleFormat, TextContext, TimeStyle};
    use lastdrop_core::jiff::tz::{self, TimeZone};
    use lastdrop_core::jiff::{SignedDuration, Timestamp, civil};
    use lastdrop_core::model::{
        AccountRef, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId, Snapshot, SourceKind,
        WindowLen,
    };
    use lastdrop_core::readout::{BuildContext, ContentForm, build_readout, compute_layout};
    use lastdrop_core::settings::{ProviderEntry, ProviderSettings, Settings};
    use lastdrop_core::stats::{self, EvalInput, EvalMemory, Sample, ServiceInput, ServiceStatus};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    use super::*;
    use crate::palette::Argb;
    use crate::render::RenderFactory;
    use crate::theme::{AccentShades, HighContrastColors};

    pub(in crate::readout) fn factory() -> RenderFactory {
        // SAFETY: initializes COM on the test thread (S_FALSE when already initialized is fine).
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        RenderFactory::new().expect("factories")
    }

    pub(in crate::readout) fn theme(dark: bool) -> ThemeInfo {
        let accent = Argb(0xFF8E_3AA7);
        ThemeInfo {
            taskbar_dark: dark,
            apps_dark: dark,
            accent: AccentShades {
                accent,
                dark1: accent,
                dark2: accent,
                light2: accent,
                light3: accent,
            },
            transparency: true,
            high_contrast: None,
            custom: Default::default(),
        }
    }

    fn mins(m: i64) -> SignedDuration {
        SignedDuration::from_mins(m)
    }

    fn zone() -> TimeZone {
        TimeZone::fixed(tz::offset(2))
    }

    /// The `now` of R§13: Sat 2026-10-10 12:42 at UTC+2 (invented data throughout).
    fn now() -> Timestamp {
        civil::date(2026, 10, 10)
            .at(12, 42, 0, 0)
            .to_zoned(zone())
            .expect("valid local time")
            .timestamp()
    }

    fn window(
        id: &str,
        names: MeterNames,
        order: u16,
        used: f64,
        resets_at: Timestamp,
        hours: i64,
        starts_on_use: bool,
    ) -> Meter {
        Meter {
            id: MeterId::new(id),
            names,
            order,
            used,
            limited: false,
            resets_at: Some(resets_at),
            window: WindowLen::Fixed(SignedDuration::from_hours(hours)),
            starts_on_use,
        }
    }

    /// Claude 5-hour going from `previous` to `used` over the last 30 minutes, resetting in
    /// `reset_in` minutes, and weekly 61 % resetting Mon 09:00. R mock 1a is (0.375, 0.42, 134),
    /// 1d is (0.79, 0.94, 228).
    pub(in crate::readout) fn model(previous: f64, used: f64, reset_in: i64) -> ReadoutModel {
        model_in(previous, used, reset_in, Language::English)
    }

    /// [`model`] in `language`, with its regional format (en-GB; sv-SE for Swedish).
    fn model_in(previous: f64, used: f64, reset_in: i64, language: Language) -> ReadoutModel {
        let now = now();
        let reset = now + mins(reset_in);
        let weekly_reset = now + mins(44 * 60 + 18);
        let snapshot = Snapshot {
            provider: ProviderId::Claude,
            account: AccountRef::primary(),
            source: SourceKind::ClaudeOAuthUsage,
            measured_at: now,
            fetched_at: now,
            meters: vec![
                window(
                    MeterId::FIVE_HOUR,
                    MeterNames::new("5h", "5-hour", "5-hour session"),
                    0,
                    used,
                    reset,
                    5,
                    true,
                ),
                window(
                    MeterId::WEEKLY,
                    MeterNames::new("Wk", "weekly", "Weekly · all models"),
                    1,
                    0.61,
                    weekly_reset,
                    7 * 24,
                    false,
                ),
            ],
            balances: Vec::new(),
            health: Health::Ok,
        };
        let mut history: BTreeMap<MeterKey, Vec<Sample>> = BTreeMap::new();
        history.insert(
            MeterKey {
                provider: ProviderId::Claude,
                account: AccountRef::primary().key,
                meter: MeterId::new(MeterId::FIVE_HOUR),
            },
            vec![
                Sample::window(now - mins(30), previous, Some(reset)),
                Sample::window(now, used, Some(reset)),
            ],
        );
        let settings = Settings {
            providers: vec![ProviderEntry::Known(ProviderSettings::new(
                ProviderId::Claude,
                true,
            ))],
            ..Settings::default()
        };
        let status = ServiceStatus {
            last_success: Some(now),
            poll_interval: Some(mins(5)),
            ..ServiceStatus::default()
        };
        let account = AccountRef::primary();
        let services = [ServiceInput {
            provider: ProviderId::Claude,
            account: &account,
            snapshot: Some(&snapshot),
            status: &status,
            fetching: false,
        }];
        let mut eval = stats::evaluate(
            &EvalInput {
                now,
                settings: &settings,
                services: &services,
                history: &history,
            },
            &mut EvalMemory::default(),
        );
        localize(&mut eval, language.strings());
        let locale = if language == Language::Swedish {
            LocaleFormat::sv_se(zone())
        } else {
            LocaleFormat::en_gb(zone())
        };
        build_readout(
            &eval,
            &BuildContext {
                text: TextContext {
                    now,
                    time_style: TimeStyle::Clock,
                    locale: &locale,
                    strings: language.strings(),
                },
                settings: &settings,
            },
        )
    }

    struct Frame {
        pixels: Vec<u8>,
        width: u32,
        layout: ReadoutLayout,
    }

    impl Frame {
        fn alpha(&self, x: i32, y: i32) -> u8 {
            let (x, y) = (x.unsigned_abs(), y.unsigned_abs());
            self.pixels[((y * self.width + x) * 4 + 3) as usize]
        }
    }

    fn render(
        model: &ReadoutModel,
        scale: f64,
        form: ReadoutForm,
        theme: &ThemeInfo,
        plate: Plate,
        floating: bool,
    ) -> Frame {
        let f = factory();
        let measurer = TextMeasurer::new(&f);
        let layout = compute_layout(&model.widths, scale, &measurer);
        let (w, h) = slot_size(&layout, form);
        let mut canvas = Canvas::new(&f, w.unsigned_abs(), h.unsigned_abs()).expect("canvas");
        draw(
            &mut canvas,
            &measurer,
            &FrameInput {
                model,
                layout: &layout,
                form,
                theme,
                plate,
                floating,
            },
        )
        .expect("frame");
        Frame {
            pixels: canvas.pixels().expect("pixels"),
            width: w.unsigned_abs(),
            layout,
        }
    }

    #[test]
    fn the_on_track_model_has_two_rows_and_the_spec_widths() {
        let m = model(0.375, 0.42, 134);
        assert_eq!(m.content, ContentForm::TwoRows);
        assert_eq!(m.rows[0].right.texts(), vec!["42%", "14:56"]);
        assert_eq!(m.rows[1].right.texts(), vec!["61%", "Mon"]);
        let frame = render(
            &m,
            1.0,
            ReadoutForm::TwoRows,
            &theme(true),
            Plate::None,
            false,
        );
        assert_eq!((frame.layout.width_rows, frame.layout.slot_h), (160, 40));
    }

    #[test]
    fn swedish_widths_come_from_the_swedish_strings() {
        // R§5.9, R§6.3: the columns are measured over the Swedish strings and the sv-SE format,
        // so no Swedish text is cut off; the readout is as wide as its widest Swedish string.
        let m = model_in(0.375, 0.42, 134, Language::Swedish);
        assert_eq!(m.content, ContentForm::TwoRows);
        let labels: Vec<&str> = m.rows.iter().map(|r| r.label.text.as_str()).collect();
        assert_eq!(labels, ["5h", "Vk"]);
        assert_eq!(m.rows[0].right.texts(), vec!["42\u{202F}%", "14:56"]);
        assert_eq!(m.rows[1].right.texts(), vec!["61\u{202F}%", "mån"]);
        let w = &m.widths;
        assert!(w.time_tokens.iter().any(|t| t == "idag"));
        assert!(w.statuses.iter().any(|t| t == "59m sedan"));
        assert!(w.statuses.iter().any(|t| t == "Logga in"));
        assert!(w.say_semibold.iter().any(|t| t == "59 min kvar"));
        assert!(w.say_semibold.iter().any(|t| t == "Åter tors"));
        assert!(w.compact_semibold.iter().any(|t| t == "99\u{202F}%"));

        let english = render(
            &model(0.375, 0.42, 134),
            1.0,
            ReadoutForm::TwoRows,
            &theme(true),
            Plate::None,
            false,
        )
        .layout;
        let sv = render(
            &m,
            1.0,
            ReadoutForm::TwoRows,
            &theme(true),
            Plate::None,
            false,
        )
        .layout;
        // English is 17 / 79 / 160 / 134 (R§6.3): `Vk` is narrower than `Wk`, and the statuses
        // `59m sedan` and `Ingen data` widen the right cell from 79 to 90.
        assert_eq!(
            (sv.label_w, sv.right_w, sv.width_rows, sv.width_compact),
            (13, 90, 167, 130)
        );
        assert_eq!((english.label_w, english.right_w), (17, 79));
        assert_eq!(sv.slot_h, english.slot_h);
    }

    #[test]
    fn bars_land_on_the_r13_pixels_at_every_scale() {
        // R§13 pace bar geometry for u .42, e .5533: (s, hair, fill end, tick).
        let m = model(0.375, 0.42, 134);
        for (scale, hair, f, k) in [
            (1.0, 1, 17, 22),
            (1.25, 1, 21, 28),
            (1.5, 1, 25, 33),
            (2.0, 2, 34, 44),
        ] {
            let frame = render(
                &m,
                scale,
                ReadoutForm::TwoRows,
                &theme(true),
                Plate::None,
                false,
            );
            let l = frame.layout;
            let g = bar_geometry(&m.rows[0].bar, BAR_W, ROW_H, scale);
            assert_eq!(
                (g.hair, g.fill_end, g.tick),
                (hair, f, Some(k)),
                "s = {scale}"
            );
            let y_track = l.y_row1 + g.y_track;
            let x = |px: i32| l.x_bar + px;
            // Inside the fill, away from the rounded ends: opaque.
            assert_eq!(frame.alpha(x(f / 2), y_track), 255, "s = {scale}");
            // The gaps are not painted: only the hit-test alpha.
            assert_eq!(frame.alpha(x(k - hair), y_track), 1, "s = {scale}");
            assert_eq!(frame.alpha(x(k + hair), y_track), 1, "s = {scale}");
            // The tick over its whole height and not beyond.
            assert_eq!(frame.alpha(x(k), l.y_row1 + g.y_tick), 255, "s = {scale}");
            let last = l.y_row1 + g.y_tick + g.tick_h - 1;
            assert_eq!(frame.alpha(x(k), last), 255, "s = {scale}");
            assert_eq!(frame.alpha(x(k), l.y_row1 + g.y_tick - 1), 1, "s = {scale}");
            // The track right of the tick: dark-theme Track #8BFFFFFF.
            assert_eq!(frame.alpha(x(g.len - 1), y_track), 0x8B, "s = {scale}");
        }
    }

    #[test]
    fn plates_fill_the_slot_or_outline_it_in_contrast_themes() {
        let m = model(0.375, 0.42, 134);
        let dark = theme(true);
        let none = render(&m, 1.0, ReadoutForm::TwoRows, &dark, Plate::None, false);
        let hover = render(&m, 1.0, ReadoutForm::TwoRows, &dark, Plate::Hover, false);
        let pressed = render(&m, 1.0, ReadoutForm::TwoRows, &dark, Plate::Pressed, false);
        // In the right padding, where nothing else is drawn.
        let (x, y) = (157, 20);
        assert_eq!(none.alpha(x, y), 1);
        // Dark SubtleSecondary #0FFFFFFF and SubtleTertiary #0AFFFFFF over the hit-test fill.
        assert!(
            (0x0F..=0x10).contains(&hover.alpha(x, y)),
            "{}",
            hover.alpha(x, y)
        );
        assert!(
            (0x0A..=0x0B).contains(&pressed.alpha(x, y)),
            "{}",
            pressed.alpha(x, y)
        );
        let mut contrast = theme(true);
        contrast.high_contrast = Some(HighContrastColors {
            window_text: Argb(0xFFFF_FFFF),
            window: Argb(0xFF20_2020),
            highlight: Argb(0xFF8E_E3F0),
            highlight_text: Argb(0xFF26_3B50),
            gray_text: Argb(0xFFA6_A6A6),
            hotlight: Argb(0xFF75_E9FC),
        });
        let outline = render(
            &m,
            1.0,
            ReadoutForm::TwoRows,
            &contrast,
            Plate::Hover,
            false,
        );
        assert_eq!(outline.alpha(x, y), 1, "no fill inside the outline");
        assert_eq!(outline.alpha(80, 0), 255, "a 1 px outline on the edge");
    }

    #[test]
    fn compact_and_floating_frames() {
        // R mock 1d: CRITICAL, 12 min left; the compact cell says `12 min` after the disc.
        let m = model(0.79, 0.94, 228);
        let texts: Vec<String> = m.rows[m.compact_row]
            .compact
            .runs
            .iter()
            .filter_map(|r| match r {
                Run::Text(t) => Some(t.text.clone()),
                Run::Glyph(_) => None,
            })
            .collect();
        assert_eq!(texts, vec!["12 min"]);
        let frame = render(
            &m,
            1.0,
            ReadoutForm::Compact,
            &theme(false),
            Plate::None,
            false,
        );
        let l = frame.layout;
        assert_eq!(i32::try_from(frame.width).ok(), Some(l.width_compact));
        // One row at y_single: the tick is drawn there, nothing in the two-row places.
        let g = bar_geometry(&m.rows[m.compact_row].bar, BAR_W, ROW_H, 1.0);
        let k = g.tick.expect("tick");
        // Light TextPrimary #E4000000.
        assert_eq!(frame.alpha(l.x_bar + k, l.y_single + g.y_tick), 0xE4);
        assert_eq!(frame.alpha(l.x_bar + k, l.y_row1 + g.y_tick), 1);
        // The critical disc right of the bar.
        assert!((l.x_right..l.x_right + 12).any(|x| frame.alpha(x, l.y_single + 8) > 128));

        let floating = render(
            &m,
            1.0,
            ReadoutForm::TwoRows,
            &theme(false),
            Plate::None,
            true,
        );
        // The light card background #FFF9F9F9 is opaque inside the card.
        assert_eq!(floating.alpha(157, 20), 255);
    }

    #[test]
    fn text_sits_on_the_mock_baselines() {
        // The last ink row of each text, from the row's top, as measured in readout-100.png and
        // readout-150.png (mock 1a, dark): `42%` and `14:56` (12 px) end on row 12 / 18, the
        // `5h` label (11 px) on 11 / 18, the `↺` (Fluent Icons 11 px) on 12 / 19. (The mock
        // centres the slot in the 47 px below the taskbar's 1 px border, so its rows start half a
        // pixel lower than R§6.1's and render a pixel lower: measured from there, the bars and
        // texts land on the native rows.) 125 % has no mock; its rows follow the same CSS line box
        // (render::line_box_baseline).
        let m = model(0.375, 0.42, 134);
        for (scale, value, label, glyph) in
            [(1.0, 12, 11, 12), (1.25, 15, 15, 16), (1.5, 18, 18, 19)]
        {
            let frame = render(
                &m,
                scale,
                ReadoutForm::TwoRows,
                &theme(true),
                Plate::None,
                false,
            );
            let l = frame.layout;
            // The last row of the first row's box with ink (alpha over a third) in `x0..x1`.
            let last_ink = |x0: i32, x1: i32| {
                (l.y_row1..l.y_row1 + l.row_h)
                    .filter(|&y| (x0..x1).any(|x| frame.alpha(x, y) > 85))
                    .max()
                    .map(|y| y - l.y_row1)
            };
            let glyph_x = l.x_right + l.time_x;
            let glyph_w = px(11.0, scale);
            let time_x = glyph_x + glyph_w + px(1.0, scale);
            assert_eq!(
                last_ink(l.x_right, l.x_right + l.value_box_w),
                Some(value),
                "s = {scale}: value"
            );
            assert_eq!(
                last_ink(time_x, l.x_right + l.right_w),
                Some(value),
                "s = {scale}: time"
            );
            assert_eq!(
                last_ink(l.x_label, l.x_label + l.label_w),
                Some(label),
                "s = {scale}: label"
            );
            assert_eq!(
                last_ink(glyph_x, glyph_x + glyph_w),
                Some(glyph),
                "s = {scale}: reset glyph"
            );
        }
    }

    #[test]
    fn rows_follow_the_form() {
        let m = model(0.375, 0.42, 134);
        let f = factory();
        let l = compute_layout(&m.widths, 1.0, &TextMeasurer::new(&f));
        let rows = |form| {
            rows_for(&m, &l, form)
                .iter()
                .map(|(r, y, compact)| (r.label.text.clone(), *y, *compact))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(ReadoutForm::TwoRows),
            vec![("5h".to_owned(), 4, false), ("Wk".to_owned(), 20, false)]
        );
        assert_eq!(
            rows(ReadoutForm::SingleRow),
            vec![("5h".to_owned(), 12, false)]
        );
        assert_eq!(rows(ReadoutForm::Compact).len(), 1);
        assert!(rows(ReadoutForm::Hidden).is_empty());
    }
}
