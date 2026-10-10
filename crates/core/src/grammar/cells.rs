//! Readout cell texts by row state (R§5.7) and the compact cell (R§5.8).

use super::TextContext;
use super::numbers::{balance_duration, percent, share_pct};
use super::strings::{Msg, text};
use super::time::{CountdownForm, age_token, countdown, runout_text, shows_countdown, time_token};
use crate::color::ColorRole;
use crate::consts::CARD_FORECAST_MAX_PCT;
use crate::consts::layout::{RESET_GLYPH, RESET_GLYPH_GAP, SEVERITY_GLYPH, SEVERITY_GLYPH_GAP};
use crate::glyph::{Icon, SeverityGlyph};
use crate::readout::{CompactCell, GlyphKind, GlyphRun, RightCell, Run, TextRun, TextStyle};
use crate::stats::{AssessedMeter, Assessment, ServiceEval, StaleReason, runway_minutes};
use jiff::Timestamp;

/// A text run.
pub(crate) fn run(text: String, style: TextStyle, role: ColorRole) -> TextRun {
    TextRun { text, style, role }
}

/// The reset glyph ↺ before a time (R§6.5: 11 px, cell 11 + 1 gap, TextSecondary).
pub(crate) fn reset_glyph() -> Run {
    Run::Glyph(GlyphRun {
        glyph: GlyphKind::Icon(Icon::Reset),
        size: RESET_GLYPH,
        cell: RESET_GLYPH,
        gap_after: RESET_GLYPH_GAP,
        role: ColorRole::TextSecondary,
    })
}

/// A severity glyph before say-text (R§6.5: 12 px, cell 12 + 2 gap).
pub(crate) fn severity_glyph(glyph: SeverityGlyph) -> Run {
    Run::Glyph(GlyphRun {
        glyph: GlyphKind::Severity(glyph),
        size: SEVERITY_GLYPH,
        cell: SEVERITY_GLYPH,
        gap_after: SEVERITY_GLYPH_GAP,
        role: ColorRole::TextPrimary,
    })
}

/// `t_out` of a meter that runs out: a window whose forecast `runs_out` (R§3.5: `t_out < R`), or
/// a balance with a forecast. `None` otherwise — also for a WARNING that hysteresis holds while
/// `p ≤ 1` (R§3.7 exit `p ≤ 0.97`): its `t_out` lies at or after the reset, and R§5.3 defines the
/// `Out` and countdown texts only for `runs_out`, so no surface may show it ([`course_pct`]
/// words that state instead).
pub(crate) fn t_out(a: &Assessment) -> Option<Timestamp> {
    match &a.meter {
        AssessedMeter::Window(w) => w.forecast.filter(|f| f.runs_out).and_then(|f| f.t_out),
        AssessedMeter::Balance(b) => b.stats.t_out,
        AssessedMeter::Loading => None,
    }
}

/// The projection `p` of a window with a valid forecast as a shown percent, capped at 99 % like
/// the card's `on course for 78% at reset` (R§8.2): what a WARNING without a run-out before the
/// reset says instead of a run-out time (`on course for 98% at the reset`).
pub(crate) fn course_pct(a: &Assessment) -> Option<u8> {
    match &a.meter {
        AssessedMeter::Window(w) => w
            .forecast
            .filter(|f| f.valid)
            .map(|f| share_pct(f.projected).min(CARD_FORECAST_MAX_PCT)),
        AssessedMeter::Balance(_) | AssessedMeter::Loading => None,
    }
}

/// `R` shown for a window meter.
pub(crate) fn resets_at(a: &Assessment) -> Option<Timestamp> {
    match &a.meter {
        AssessedMeter::Window(w) => w.resets_at,
        AssessedMeter::Balance(_) | AssessedMeter::Loading => None,
    }
}

/// `pct` of a meter; 0 while loading.
pub(crate) fn pct(a: &Assessment) -> u8 {
    match &a.meter {
        AssessedMeter::Window(w) => w.pct,
        AssessedMeter::Balance(b) => b.pct,
        AssessedMeter::Loading => 0,
    }
}

/// The frozen reading of a stale window meter was used up (R§5.7: it shows `Back 16:30` from
/// x = 0 instead of a `100%` that does not fit the VALUE box).
pub(crate) fn frozen_exhausted(a: &Assessment) -> bool {
    matches!(a.state, crate::stats::RowState::Stale(_))
        && match &a.meter {
            AssessedMeter::Window(w) => w.exhausted,
            AssessedMeter::Balance(b) => b.stats.exhausted,
            AssessedMeter::Loading => false,
        }
}

/// `Back TIME`, or `Empty` without a reset time (R§5.1, R§5.7).
pub(crate) fn back_text(a: &Assessment, cx: &TextContext<'_>) -> String {
    match resets_at(a) {
        Some(r) => text(cx.strings, Msg::Back, &[("time", &time_token(r, cx))]),
        None => cx.strings.template(Msg::Empty).to_owned(),
    }
}

/// The run-out text of a WARNING or CRITICAL_RUNWAY row (R§5.3); `pct` when the meter does not
/// run out before its reset (a WARNING held by hysteresis, [`t_out`]), as the compact cell shows.
fn say_runout(a: &Assessment, critical: bool, cx: &TextContext<'_>) -> String {
    match t_out(a) {
        Some(t) => runout_text(t, critical, cx),
        None => percent(pct(a), cx.locale),
    }
}

/// The REASON token of a stale service (R§5.1).
pub(crate) fn reason_text(reason: StaleReason, cx: &TextContext<'_>) -> String {
    let msg = match reason {
        StaleReason::SignIn => Msg::ReasonSignIn,
        StaleReason::Paused => Msg::ReasonPaused,
        StaleReason::Offline => Msg::ReasonOffline,
        StaleReason::NoData => Msg::ReasonNoData,
    };
    cx.strings.template(msg).to_owned()
}

/// The status of a stale row (R§5.7): REASON (`Sign in` in AccentText, others in the stale
/// colour) or AGE. Two rows take REASON then AGE; one row shows `Sign in` or `Paused`, else AGE.
pub(crate) fn stale_status(
    reason: StaleReason,
    service: &ServiceEval,
    pos: RowPosition,
    cx: &TextContext<'_>,
) -> TextRun {
    let reason_run = |r: StaleReason| {
        let role = if r == StaleReason::SignIn {
            ColorRole::AccentText
        } else {
            ColorRole::Stale
        };
        run(reason_text(r, cx), TextStyle::ROW, role)
    };
    let age = || {
        let last = service.status.last_success.or(service.reading_at);
        let age = last.map_or_else(
            || cx.strings.template(Msg::NoReset).to_owned(),
            |t| age_token(t, cx),
        );
        run(age, TextStyle::ROW, ColorRole::Stale)
    };
    if pos.rows_in_service >= 2 {
        if pos.index_in_service == 0 {
            reason_run(reason)
        } else {
            age()
        }
    } else {
        match reason {
            StaleReason::SignIn | StaleReason::Paused => reason_run(reason),
            StaleReason::Offline | StaleReason::NoData => age(),
        }
    }
}

/// Where a row sits among the readout rows of its service. A stale service's rows take, in
/// order, REASON then AGE (R§5.7 "STALE status, per service"), so the cell depends on it.
///
/// When the service's other shown row is a frozen used-up reading (which shows `Back 16:30` and
/// no status), the caller passes `rows_in_service: 1` for this row: it then takes the status a
/// one-row service shows (R§5.7 "the status goes to the service's other row").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RowPosition {
    /// 0-based index among the shown rows of this service.
    pub index_in_service: usize,
    /// How many rows of this service are shown.
    pub rows_in_service: usize,
}

/// The right cell of a row (R§5.7). `RIGHT_W` wide; sub-columns: the VALUE box (24 px,
/// right-aligned at x = 0) and time/status from x = 30; say-text from x = 0.
///
/// | State            | Right cell                                                                 |
/// |------------------|----------------------------------------------------------------------------|
/// | LOADING          | first row of the service: `Reading…` from x = 0 (TextSecondary); other rows: `–` in the VALUE box |
/// | OK               | `42%` + `↺14:56`                                                           |
/// | OK, no reset     | `42%` + `–`                                                                |
/// | IDLE             | `0%` + `–`                                                                 |
/// | OK, balance      | `62%` + `≈9d` (`–` without a forecast)                                     |
/// | CRITICAL_LEVEL   | `92%` **semibold** + `↺16:30`                                              |
/// | WARNING          | ▲ + `Out 15:20` (or `45 min left` when N ≤ 59)                             |
/// | CRITICAL_RUNWAY  | ● + countdown, always                                                      |
/// | EXHAUSTED        | ● + `Back 16:30` (no reset time, or balance: ● + `Empty`)                  |
/// | JUST_RESET       | `0%` + `↺7d` (the new `R'`)                                                |
/// | STALE            | `42%` (stale colour) + REASON or AGE                                       |
///
/// STALE details: a one-row service shows `Sign in` (SIGN_IN) or `Paused` (PAUSED), otherwise
/// AGE; a row whose frozen reading was EXHAUSTED shows `Back 16:30` from x = 0, regular weight,
/// stale colour, no glyph, and the status moves to the service's other row (one-row service: no
/// status). `Sign in` is AccentText; other statuses the stale colour.
pub fn right_cell(
    a: &Assessment,
    service: &ServiceEval,
    pos: RowPosition,
    cx: &TextContext<'_>,
) -> RightCell {
    use crate::stats::RowState as S;
    let value =
        |role: ColorRole, style: TextStyle| Some(run(percent(pct(a), cx.locale), style, role));
    let dash = || {
        run(
            cx.strings.template(Msg::NoReset).to_owned(),
            TextStyle::ROW,
            ColorRole::TextSecondary,
        )
    };
    let reset_time = || match resets_at(a) {
        Some(r) => vec![
            reset_glyph(),
            Run::Text(run(
                time_token(r, cx),
                TextStyle::ROW,
                ColorRole::TextPrimary,
            )),
        ],
        None => vec![Run::Text(dash())],
    };
    let say = |glyph: SeverityGlyph, words: String| {
        vec![
            severity_glyph(glyph),
            Run::Text(run(words, TextStyle::ROW_SEMIBOLD, ColorRole::TextPrimary)),
        ]
    };
    match a.state {
        S::Loading => {
            if pos.index_in_service == 0 {
                RightCell {
                    say: vec![Run::Text(run(
                        cx.strings.template(Msg::Loading).to_owned(),
                        TextStyle::ROW,
                        ColorRole::TextSecondary,
                    ))],
                    ..RightCell::default()
                }
            } else {
                RightCell {
                    value: Some(dash()),
                    ..RightCell::default()
                }
            }
        }
        S::Ok | S::JustReset => RightCell {
            value: value(ColorRole::TextPrimary, TextStyle::ROW),
            time: reset_time(),
            say: Vec::new(),
        },
        S::OkNoReset | S::Idle => RightCell {
            value: value(ColorRole::TextPrimary, TextStyle::ROW),
            time: vec![Run::Text(dash())],
            say: Vec::new(),
        },
        S::OkBalance => {
            let time = match &a.meter {
                AssessedMeter::Balance(b) => match balance_duration(&b.stats, cx.strings) {
                    Some(d) => vec![
                        Run::Text(run(
                            cx.strings.template(Msg::BalanceApprox).to_owned(),
                            TextStyle::ROW,
                            ColorRole::TextSecondary,
                        )),
                        Run::Text(run(d, TextStyle::ROW, ColorRole::TextPrimary)),
                    ],
                    None => vec![Run::Text(dash())],
                },
                _ => vec![Run::Text(dash())],
            };
            RightCell {
                value: value(ColorRole::TextPrimary, TextStyle::ROW),
                time,
                say: Vec::new(),
            }
        }
        S::CriticalLevel => RightCell {
            value: value(ColorRole::TextPrimary, TextStyle::ROW_SEMIBOLD),
            time: reset_time(),
            say: Vec::new(),
        },
        S::Warning => RightCell {
            say: say(SeverityGlyph::Warning, say_runout(a, false, cx)),
            ..RightCell::default()
        },
        S::CriticalRunway => RightCell {
            say: say(SeverityGlyph::Critical, say_runout(a, true, cx)),
            ..RightCell::default()
        },
        S::Exhausted => RightCell {
            say: say(SeverityGlyph::Critical, back_text(a, cx)),
            ..RightCell::default()
        },
        S::Stale(reason) => {
            if frozen_exhausted(a) {
                // `Back 16:30` from x = 0, regular, stale colour, no glyph, no status.
                RightCell {
                    say: vec![Run::Text(run(
                        back_text(a, cx),
                        TextStyle::ROW,
                        ColorRole::Stale,
                    ))],
                    ..RightCell::default()
                }
            } else {
                RightCell {
                    value: value(ColorRole::Stale, TextStyle::ROW),
                    time: vec![Run::Text(stale_status(reason, service, pos, cx))],
                    say: Vec::new(),
                }
            }
        }
    }
}

/// The compact cell (R§5.8), left-aligned: OK `61%` · IDLE `0%` · STALE `61%` (stale colour) ·
/// WARNING ▲ `78%` semibold · any row that shows a countdown in the two-row form ▲/● + COUNTDOWN
/// without "left" (`12 min`, `6 h`) semibold · CRITICAL_LEVEL ● `92%` semibold · EXHAUSTED
/// ● `Empty` semibold · LOADING `–`.
pub fn compact_cell(a: &Assessment, cx: &TextContext<'_>) -> CompactCell {
    use crate::stats::RowState as S;
    let plain = |role: ColorRole| CompactCell {
        runs: vec![Run::Text(run(
            percent(pct(a), cx.locale),
            TextStyle::ROW,
            role,
        ))],
    };
    let trouble = |glyph: SeverityGlyph, words: String| CompactCell {
        runs: vec![
            severity_glyph(glyph),
            Run::Text(run(words, TextStyle::ROW_SEMIBOLD, ColorRole::TextPrimary)),
        ],
    };
    // The countdown the two-row form would show, without "left".
    let countdown_words = |critical: bool| {
        t_out(a)
            .map(|t| runway_minutes(cx.now, t))
            .filter(|&n| shows_countdown(n, critical))
            .map(|n| countdown(n, CountdownForm::Compact, cx.strings))
    };
    match a.state {
        S::Loading => CompactCell {
            runs: vec![Run::Text(run(
                cx.strings.template(Msg::NoReset).to_owned(),
                TextStyle::ROW,
                ColorRole::TextSecondary,
            ))],
        },
        S::Ok | S::OkNoReset | S::Idle | S::OkBalance | S::JustReset => {
            plain(ColorRole::TextPrimary)
        }
        S::Stale(_) => plain(ColorRole::Stale),
        S::Warning => trouble(
            SeverityGlyph::Warning,
            countdown_words(false).unwrap_or_else(|| percent(pct(a), cx.locale)),
        ),
        S::CriticalRunway => trouble(
            SeverityGlyph::Critical,
            countdown_words(true).unwrap_or_else(|| percent(pct(a), cx.locale)),
        ),
        S::CriticalLevel => trouble(SeverityGlyph::Critical, percent(pct(a), cx.locale)),
        S::Exhausted => trouble(
            SeverityGlyph::Critical,
            cx.strings.template(Msg::Empty).to_owned(),
        ),
    }
}
