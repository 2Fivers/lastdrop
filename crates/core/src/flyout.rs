//! The flyout as a model (flyout-spec). All text and geometry inputs are produced here by the
//! same functions as the readout; Slint only lays out what it is given (F§0: "Nothing in `.slint`
//! formats a number or a time"). The Slint structs in crates/app/ui/flyout.slint mirror these
//! types (F§11); the app's wiring converts them, running [`crate::readout::bar_geometry`] and
//! [`spark_geometry`] at the window's scale.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};
use serde::{Deserialize, Serialize};

use crate::color::ColorRole;
use crate::consts::layout::{SPARK_COL_W, SPARK_H, SPARK_W};
use crate::consts::{SPARK_BALANCE_FLOOR_SHARE, SPARK_DAYS, SPARK_WINDOW_FLOOR_PP};
use crate::glyph::{ProviderGlyph, SeverityGlyph};
use crate::grammar::TextContext;
use crate::grammar::a11y::{row_sentence, runout_point, unit_word};
use crate::grammar::cells::{course_pct, pct, resets_at, t_out};
use crate::grammar::numbers::{amount, balance_forecast_long, percent};
use crate::grammar::strings::{Msg, text};
use crate::grammar::time::{
    CountdownForm, age_long, card_date, card_time, countdown, long_duration, runout_text,
    shown_age, shown_duration, spoken_duration,
};
use crate::model::{MeterId, MeterKey, SourceKey, Unit, catalog};
use crate::readout::bar::{BarInput, Seg, bar_input};
use crate::readout::build::BuildContext;
use crate::readout::hovercard::{balance, before_reset, capitalise, last_reading, window};
use crate::stats::{
    Assessment, DisplayClass, Evaluation, FailureKind, Freshness, HistoryView, LiveChange,
    ResetView, RowState, Sample, ServiceEval, StaleReason, candidate, instance_of, runway_minutes,
    same_window_instance,
};
use crate::util::{duration_from_hours, round_i32, sub};

/// Days before today in the sparkline (14 days ending today).
const SPARK_DAYS_BEFORE_TODAY: i64 = SPARK_DAYS as i64 - 1;

/// Index of `date` among the sparkline days that start at `first`, if it is one of them.
fn day_index(first: Date, date: Date) -> Option<usize> {
    let days = first.duration_until(date).as_hours().div_euclid(24);
    usize::try_from(days).ok().filter(|&i| i < SPARK_DAYS)
}

/// The first of the sparkline's local days (today − 13).
fn first_spark_day(now: Timestamp, tz: &TimeZone) -> Date {
    let today = now.to_zoned(tz.clone()).date();
    today
        .checked_sub(Span::new().days(SPARK_DAYS_BEFORE_TODAY))
        .unwrap_or(today)
}

/// The colour family of a meter row, bar and say-glyph (F§11 `Tone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    /// Calm: AccentFill bar, no glyph.
    Calm,
    /// Caution: WARNING.
    Caution,
    /// Critical: CRITICAL and EXHAUSTED.
    Critical,
    /// Stale: dashed, stale colour.
    Stale,
}

/// The header verdict (F§3): worst first (EXHAUSTED > CRITICAL > WARNING > STALE > OK), with
/// ` · N more` when more than one item is not OK.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// In trouble: severity glyph 12 + 4 + text.
    pub glyph: Option<SeverityGlyph>,
    /// `All limits last until their reset`, `Claude 5-hour runs out at 15:20`, … (F§3 table).
    pub text: String,
}

/// What a message-line hyperlink does (F§4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CardAction {
    /// `Open sign-in…`: the vendor's own login in a terminal with this account's config folder
    /// (`claude` / `codex login`), ROADMAP "Add an account". Hides the flyout.
    OpenSignIn,
    /// `Resume`: end the pause.
    Resume,
    /// `Retry`: refresh this service now (respecting `min_interval` and back-off).
    Retry,
    /// `Enter key…`: open the settings window at this service's API key (services without a
    /// vendor sign-in, `ProviderId::client_name` is `None`: Meshy). Hides the flyout.
    OpenSettings,
}

/// Why a card shows a message line (F§4.3). Slint `CardData.msg-kind` is this enum's position
/// + 1 (0 = no message): 1 Offline, 2 NoData, 3 SignIn, 4 Paused, 5 FirstFetchFailed, 6 ApiKey.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MessageKind {
    /// OFFLINE: **Offline** · showing 12:42 · next try 13:30.
    Offline,
    /// NO_DATA: **Not answering** · showing 12:42 · next try 13:30.
    NoData,
    /// SIGN_IN: **Sign-in expired.** Lastdrop never signs in for you. + `Open sign-in…`.
    SignIn,
    /// PAUSED: **Paused** · showing 12:42 + `Resume`.
    Paused,
    /// The first fetch failed: **Couldn't read Claude** · {short detail} + `Retry`.
    FirstFetchFailed,
    /// SIGN_IN of a service signed in with an API key (Meshy: key missing or rejected):
    /// **API key missing or rejected.** + `Enter key…` (informational glyph). Not in F§4.3,
    /// whose sign-in action opens a vendor login that such services do not have.
    ApiKey,
}

/// A hyperlink in a message line or the footer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkModel {
    /// Its text (AccentText); also its accessible name.
    pub label: String,
    /// What it does.
    pub action: CardAction,
}

/// The optional message line of a card (F§4.3): margin-top 8; glyph 12 + 6 + text (Small 12/16
/// TextSecondary, first phrase semibold TextPrimary); wraps to at most 2 lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageLine {
    /// Why.
    pub kind: MessageKind,
    /// Stale (U+F140), informational (sign-in) or critical (first fetch failed).
    pub glyph: SeverityGlyph,
    /// The semibold first phrase.
    pub lead: String,
    /// The rest, starting with its separator (` · showing 12:42 · next try 13:30`).
    pub text: String,
    /// Optional hyperlink.
    pub action: Option<LinkModel>,
}

/// What sits right of the meter name (F§4.4 "Right cluster"; F§11 `right-kind` 0, 1, 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RightKind {
    /// U+E777 12 px + 4 + text, TextSecondary (`Resets 14:56`).
    Reset,
    /// Severity glyph + 4 + semibold TextPrimary text (`Out 15:20`, `12 min left`, `Back 16:30`).
    Say,
    /// No glyph, text in TextSecondary (`Starts with your next use`, `No reset time given`,
    /// stale `Resets 14:56`); LOADING: empty text. F§11's `right-kind` 2 ("none").
    Plain,
}

/// The right cluster of a meter row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RightCluster {
    /// Layout.
    pub kind: RightKind,
    /// The severity glyph for [`RightKind::Say`].
    pub glyph: Option<SeverityGlyph>,
    /// The text.
    pub text: String,
}

/// A window meter row (F§4.4): name row 20 (name, right cluster, 12, value min 36), 4, bar
/// 310 × 9, optional trouble line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeterRowModel {
    /// The meter.
    pub key: MeterKey,
    /// Meter `name`, Text 14/20, ellipsis.
    pub name: String,
    /// `42%` (Text 14/20 semibold; `100%` allowed here); LOADING `–`.
    pub value: String,
    /// STALE: the value in TextSecondary.
    pub value_dim: bool,
    /// Right of the name.
    pub right: RightCluster,
    /// Trouble line (margin-top 4, 12/16 TextSecondary, one line, ellipsis) in JUST_RESET,
    /// WARNING, CRITICAL and EXHAUSTED (F§4.4 table).
    pub line: Option<String>,
    /// Colour family.
    pub tone: Tone,
    /// Bar inputs (calm fill AccentFill).
    pub bar: BarInput,
    /// Accessible label: the row sentence (R§11).
    pub a11y: String,
    /// Accessible value `pct` (min 0, max 100); `None` while LOADING.
    pub pct: Option<u8>,
}

/// The balance body (F§4.5): row 28 (amount 20/28, 6, unit 14/20, forecast right), 2, row 16
/// (spent · burn left with ellipsis, 8, until right). No bar: no window, no pace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceBodyModel {
    /// The meter.
    pub key: MeterKey,
    /// Locale-grouped amount (`1,240`), Display 20/28 semibold.
    pub amount: String,
    /// Provider unit (`credits`), Text 14/20 TextSecondary.
    pub unit: String,
    /// `≈ 9 days left`, …; EXHAUSTED `Empty`.
    pub forecast: String,
    /// WARNING/CRITICAL/EXHAUSTED: severity glyph 12 + 4 + semibold.
    pub forecast_glyph: Option<SeverityGlyph>,
    /// `62% of last top-up used · 136 a day` (` · 136 a day` omitted without a forecast).
    pub spent: String,
    /// `until ≈ Mon 19 Oct`, never cut; `None` without a forecast.
    pub until: Option<String>,
    /// Accessible text (F§10).
    pub a11y: String,
}

/// The body of a card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CardBody {
    /// Window meters in provider order (hidden meters omitted, F§2.2).
    Meters(Vec<MeterRowModel>),
    /// A balance service.
    Balance(BalanceBodyModel),
}

/// What a sparkline counts (F§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SparkKind {
    /// Window services: the history meter (longest all-models window: Claude weekly all models,
    /// Codex weekly); `v_k` = sum of positive increases of `u` between consecutive readings of
    /// the same window instance within local day k, in percentage points; a drop at a reset is
    /// ignored. Floor 10 points.
    Window,
    /// Balance services: sum of balance decreases within local day k (top-ups ignored), in the
    /// provider unit. Floor 5 % of `ref`.
    Balance,
}

/// A 14-day sparkline (F§6), DPI-independent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SparkModel {
    /// `v_k` per local day, oldest first, today last; `None` = no data (empty slot).
    pub values: [Option<f64>; SPARK_DAYS],
    /// The scale floor: `max = max(max_k v_k, floor)`.
    pub floor: f64,
    /// A stale service: all columns TextTertiary (no accent for today).
    pub dim: bool,
    /// Accessible label (`Weekly limit used per day, last 14 days: today 6 percent, …`).
    pub a11y: String,
}

/// One sparkline column, device px, relative to the sparkline box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SparkColumn {
    /// x of the column.
    pub x: i32,
    /// Height, from the bottom edge of the box (it covers the baseline there).
    pub h: i32,
    /// Today's column (AccentFill unless dim).
    pub today: bool,
}

/// The sparkline in device px (F§6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SparkGeometry {
    /// `round(55 s)`.
    pub box_w: i32,
    /// `round(16 s)`.
    pub box_h: i32,
    /// `round(3 s)`: column width.
    pub col_w: i32,
    /// `hair = max(1, floor(s))`: gap and baseline height.
    pub hair: i32,
    /// The baseline across all 14 slots, `hair` high at the bottom, TextDisabled.
    pub baseline: Seg,
    /// Columns with `v > 0`.
    pub columns: Vec<SparkColumn>,
}

/// F§6 geometry: 14 columns `round(3 s)` wide with `hair` gaps, right-aligned in the
/// `round(55 s)` box (oldest left, today right); column height
/// `round(v / max × (16 s − hair))`, at least one `hair` when `v > 0`; `v = 0` or no data → no
/// column.
pub fn spark_geometry(spark: &SparkModel, scale: f64) -> SparkGeometry {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let box_w = round_i32(f64::from(SPARK_W) * s);
    let box_h = round_i32(f64::from(SPARK_H) * s);
    let col_w = round_i32(f64::from(SPARK_COL_W) * s);
    // `floor` of a positive finite scale fits easily in an i32.
    let hair = (s.floor() as i32).max(1);
    let n = i32::try_from(SPARK_DAYS).unwrap_or(14);
    let total = n * col_w + (n - 1) * hair;
    let x0 = box_w - total;
    let max = spark
        .values
        .iter()
        .flatten()
        .copied()
        .filter(|v| v.is_finite())
        .fold(spark.floor, f64::max);
    let columns = spark
        .values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            // `v > 0` and `max > 0`, false for NaN as well.
            let v = (*v).filter(|v| *v > 0.0 && max > 0.0)?;
            let i = i32::try_from(i).ok()?;
            let h = round_i32(v / max * f64::from(box_h - hair)).max(hair);
            Some(SparkColumn {
                x: x0 + i * (col_w + hair),
                h,
                today: i == n - 1,
            })
        })
        .collect();
    SparkGeometry {
        box_w,
        box_h,
        col_w,
        hair,
        baseline: Seg { x0, x1: x0 + total },
        columns,
    }
}

/// `v_k` for the 14 local days ending today (F§6), from the history of `key`.
///
/// An increase (window) or decrease (balance) between two consecutive readings counts on the
/// local day of the later reading; a day with at least one reading has a value (`Some(0.0)` when
/// nothing was used), a day without readings has none.
pub fn spark_values(
    history: &dyn HistoryView,
    key: &MeterKey,
    kind: SparkKind,
    now: Timestamp,
    tz: &TimeZone,
) -> [Option<f64>; SPARK_DAYS] {
    let mut values = [None; SPARK_DAYS];
    let first = first_spark_day(now, tz);
    let start = first
        .to_zoned(tz.clone())
        .map(|z| z.timestamp())
        .unwrap_or_else(|_| sub(now, SignedDuration::from_hours(24 * SPARK_DAYS as i64)));
    // One day more, for the reading before the first one of the first day.
    let samples = history.samples(key, sub(start, SignedDuration::from_hours(24)));
    let index = |t: Timestamp| day_index(first, t.to_zoned(tz.clone()).date());
    for sample in samples.iter().filter(|s| s.t >= start && s.t <= now) {
        if let Some(i) = index(sample.t) {
            values[i].get_or_insert(0.0);
        }
    }
    // Window instances per `instance_of`: a reading without a reset time inside a known instance
    // belongs to it, so its increases count.
    let mut previous: Option<(Sample, Option<Timestamp>)> = None;
    for &b in samples {
        let b_instance = instance_of(previous.and_then(|(_, i)| i), &b);
        if let Some((a, a_instance)) = previous
            && b.t >= start
            && b.t <= now
        {
            let used = match kind {
                SparkKind::Window if same_window_instance(a_instance, b_instance) => {
                    (b.value - a.value).max(0.0) * 100.0
                }
                SparkKind::Window => 0.0,
                SparkKind::Balance => (a.value - b.value).max(0.0),
            };
            if let Some(i) = index(b.t)
                && used.is_finite()
            {
                *values[i].get_or_insert(0.0) += used;
            }
        }
        previous = Some((b, b_instance));
    }
    values
}

/// The meter whose history the sparkline shows (F§6): a window service's longest all-models
/// window (`weekly` when the service has one), a balance service's first balance.
fn spark_meter(service: &ServiceEval) -> (MeterKey, SparkKind, f64) {
    let key = |id: MeterId| MeterKey {
        provider: service.source.provider,
        account: service.source.account.clone(),
        meter: id,
    };
    let is_balance = |a: &&Assessment| candidate(a).is_balance;
    if service.meters.iter().all(|a| is_balance(&a))
        && let Some(b) = service.meters.first()
    {
        let floor = balance(b).map_or(0.0, |b| b.stats.reference * SPARK_BALANCE_FLOOR_SHARE);
        return (b.key.clone(), SparkKind::Balance, floor);
    }
    let windows: Vec<&Assessment> = service.meters.iter().filter(|a| !is_balance(a)).collect();
    let chosen = windows
        .iter()
        .find(|a| a.key.meter.0 == MeterId::WEEKLY)
        .or_else(|| {
            windows
                .iter()
                .max_by_key(|a| window(a).and_then(|w| w.window).unwrap_or_default())
        });
    if let Some(a) = chosen {
        return (a.key.clone(), SparkKind::Window, SPARK_WINDOW_FLOOR_PP);
    }
    // No meters (the first fetch failed): the catalogue's history meter.
    let defs = catalog(service.source.provider);
    let def = defs
        .iter()
        .find(|d| d.id == MeterId::WEEKLY)
        .or_else(|| defs.first());
    match def {
        Some(d) if d.kind == crate::model::MeterKind::Balance => {
            (key(d.meter_id()), SparkKind::Balance, 0.0)
        }
        Some(d) => (key(d.meter_id()), SparkKind::Window, SPARK_WINDOW_FLOOR_PP),
        None => (
            key(MeterId::new(MeterId::WEEKLY)),
            SparkKind::Window,
            SPARK_WINDOW_FLOOR_PP,
        ),
    }
}

/// The sparkline of a card (F§6), with its accessible label.
fn spark_model(
    service: &ServiceEval,
    history: &dyn HistoryView,
    cx: &TextContext<'_>,
) -> SparkModel {
    let (key, kind, floor) = spark_meter(service);
    let values = spark_values(history, &key, kind, cx.now, &cx.locale.tz);
    let whole = |v: f64| crate::util::round_half_up(v).max(0.0) as i64;
    let today = whole(values[SPARK_DAYS - 1].unwrap_or(0.0));
    // The highest day; the latest of equal days.
    let (max_i, max_v) = values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (i, v)))
        .fold((SPARK_DAYS - 1, 0.0), |best, (i, v)| {
            if v >= best.1 { (i, v) } else { best }
        });
    let with_data: Vec<f64> = values.iter().flatten().copied().collect();
    let avg = if with_data.is_empty() {
        0.0
    } else {
        with_data.iter().sum::<f64>() / with_data.len() as f64
    };
    let first = first_spark_day(cx.now, &cx.locale.tz);
    let day_date = first
        .checked_add(Span::new().days(i64::try_from(max_i).unwrap_or(0)))
        .unwrap_or(first);
    let weekday = usize::try_from(day_date.weekday().to_monday_zero_offset()).unwrap_or(0) % 7;
    let day = cx.locale.weekdays_full[weekday].clone();
    let args = |name: &'static str, value: String| (name, value);
    let numbers = [
        args("today", today.to_string()),
        args("max", whole(max_v).to_string()),
        args("day", day),
        args("avg", whole(avg).to_string()),
    ];
    let mut refs: Vec<(&str, &str)> = numbers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let a11y = match kind {
        SparkKind::Window => {
            let meter = service
                .meters
                .iter()
                .find(|a| a.key == key)
                .map(|a| capitalise(&a.names.short_name))
                .or_else(|| {
                    catalog(key.provider)
                        .iter()
                        .find(|d| d.id == key.meter.0)
                        .map(|d| capitalise(d.short_name))
                })
                .unwrap_or_default();
            refs.push(("meter", &meter));
            text(cx.strings, Msg::FlyoutSparkWindow, &refs)
        }
        SparkKind::Balance => {
            let unit = service
                .meters
                .iter()
                .find(|a| a.key == key)
                .and_then(balance)
                .map_or_else(
                    || unit_word(&Unit::Credits, cx.strings),
                    |b| unit_word(&b.unit, cx.strings),
                );
            let unit = capitalise(&unit);
            refs.push(("unit", &unit));
            text(cx.strings, Msg::FlyoutSparkBalance, &refs)
        }
    };
    SparkModel {
        values,
        floor,
        dim: matches!(service.freshness, Freshness::Stale(_)),
        a11y,
    }
}

/// The header verdict (F§3): worst first, ` · N more` when more than one item is not OK. Items
/// are the meters in trouble and the stale services (all paused services count once).
fn verdict(eval: &Evaluation, cx: &TextContext<'_>) -> Verdict {
    let s = cx.strings;
    enum Item<'a> {
        Meter(&'a ServiceEval, &'a Assessment),
        Stale(&'a ServiceEval, StaleReason),
    }
    let rank_of = |k: &MeterKey| {
        eval.ranked
            .iter()
            .position(|r| r == k)
            .unwrap_or(usize::MAX)
    };
    let mut items: Vec<(DisplayClass, usize, Item<'_>)> = Vec::new();
    let mut paused_counted = false;
    for service in &eval.services {
        if let Freshness::Stale(reason) = service.freshness {
            if reason == StaleReason::Paused {
                if paused_counted {
                    continue;
                }
                paused_counted = true;
            }
            let order = usize::MAX / 2 + service.service_order;
            items.push((DisplayClass::Stale, order, Item::Stale(service, reason)));
            continue;
        }
        for a in &service.meters {
            if a.class >= DisplayClass::Warning {
                items.push((a.class, rank_of(&a.key), Item::Meter(service, a)));
            }
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let Some((_, _, worst)) = items.first() else {
        let all_loading = !eval.services.is_empty() && eval.services.iter().all(|s| s.loading);
        // "Last until their reset" needs reset times: without any (Claude Desktop's history
        // alone) the verdict claims none.
        let windows: Vec<_> = eval
            .services
            .iter()
            .flat_map(|s| s.meters.iter())
            .filter_map(window)
            .collect();
        let no_reset = !windows.is_empty()
            && windows.iter().all(|w| w.resets_at.is_none())
            && eval
                .services
                .iter()
                .flat_map(|s| s.meters.iter())
                .all(|a| balance(a).is_none());
        let msg = if all_loading {
            Msg::FlyoutVerdictLoading
        } else if no_reset {
            Msg::FlyoutVerdictOkNoReset
        } else {
            Msg::FlyoutVerdictOk
        };
        return Verdict {
            glyph: None,
            text: s.template(msg).to_owned(),
        };
    };
    let (glyph, mut words) = match worst {
        Item::Stale(service, reason) => {
            let svc = service.display_name();
            match reason {
                StaleReason::SignIn => (
                    SeverityGlyph::Info,
                    text(s, Msg::FlyoutVerdictSignIn, &[("service", &svc)]),
                ),
                StaleReason::Paused => (
                    SeverityGlyph::Stale,
                    s.template(Msg::FlyoutVerdictPaused).to_owned(),
                ),
                StaleReason::Offline | StaleReason::NoData | StaleReason::OldReading => {
                    let age = last_reading(service)
                        .map(|t| long_duration(shown_age(cx.now, t), s))
                        .unwrap_or_default();
                    (
                        SeverityGlyph::Stale,
                        text(
                            s,
                            Msg::FlyoutVerdictStale,
                            &[("service", &svc), ("age", &age)],
                        ),
                    )
                }
            }
        }
        Item::Meter(service, a) => {
            let label = text(
                s,
                Msg::CardAlsoLine,
                &[
                    ("service", &service.display_name()),
                    ("short_name", &a.names.short_name),
                ],
            );
            let pct_text = percent(pct(a), cx.locale);
            let level = || {
                text(
                    s,
                    Msg::FlyoutVerdictLevel,
                    &[("label", &label), ("pct", &pct_text)],
                )
            };
            match a.state {
                RowState::Exhausted => (
                    SeverityGlyph::Critical,
                    match resets_at(a) {
                        Some(r) => text(
                            s,
                            Msg::FlyoutVerdictExhausted,
                            &[("label", &label), ("time", &card_time(r, cx))],
                        ),
                        None => level(),
                    },
                ),
                RowState::CriticalRunway => (
                    SeverityGlyph::Critical,
                    match t_out(a) {
                        Some(t) => {
                            let n = runway_minutes(cx.now, t);
                            let c = countdown(n, CountdownForm::Compact, s);
                            text(
                                s,
                                Msg::FlyoutVerdictRunway,
                                &[("label", &label), ("countdown", &c)],
                            )
                        }
                        None => level(),
                    },
                ),
                RowState::Warning => (
                    SeverityGlyph::Warning,
                    match (t_out(a), course_pct(a)) {
                        (Some(t), _) => text(
                            s,
                            Msg::FlyoutVerdictWarning,
                            &[("label", &label), ("time", &runout_point(a, t, cx))],
                        ),
                        // Held by hysteresis without a run-out before the reset (R§5.3).
                        (None, Some(p)) => text(
                            s,
                            Msg::FlyoutVerdictWarningCourse,
                            &[("label", &label), ("p", &percent(p, cx.locale))],
                        ),
                        (None, None) => level(),
                    },
                ),
                _ => (SeverityGlyph::Critical, level()),
            }
        }
    };
    if items.len() > 1 {
        words.push_str(&text(
            s,
            Msg::FlyoutVerdictMore,
            &[("n", &(items.len() - 1).to_string())],
        ));
    }
    Verdict {
        glyph: Some(glyph),
        text: words,
    }
}

/// The message line of a card (F§4.3), if it needs one.
fn message_line(service: &ServiceEval, cx: &TextContext<'_>) -> Option<MessageLine> {
    let s = cx.strings;
    let sep = s.template(Msg::FlyoutMsgSep);
    let svc = service.display_name();
    let api_key = service.source.provider.client_name().is_none();
    let sign_in = || {
        Some(if api_key {
            MessageLine {
                kind: MessageKind::ApiKey,
                glyph: SeverityGlyph::Info,
                lead: s.template(Msg::FlyoutMsgApiKeyLead).to_owned(),
                text: String::new(),
                action: Some(LinkModel {
                    label: s.template(Msg::FlyoutMsgApiKeyAction).to_owned(),
                    action: CardAction::OpenSettings,
                }),
            }
        } else {
            MessageLine {
                kind: MessageKind::SignIn,
                glyph: SeverityGlyph::Info,
                lead: s.template(Msg::FlyoutMsgSignInLead).to_owned(),
                text: format!(" {}", s.template(Msg::FlyoutMsgSignInText)),
                action: Some(LinkModel {
                    label: s.template(Msg::FlyoutMsgSignInAction).to_owned(),
                    action: CardAction::OpenSignIn,
                }),
            }
        })
    };
    if service.first_fetch_failed {
        let auth = service.status.sign_in_required
            || matches!(service.status.last_failure, Some((_, FailureKind::Auth)));
        if auth {
            return sign_in();
        }
        let text_part = service
            .status
            .failure_detail
            .as_ref()
            .map(|d| format!("{sep}{d}"))
            .unwrap_or_default();
        return Some(MessageLine {
            kind: MessageKind::FirstFetchFailed,
            glyph: SeverityGlyph::Critical,
            lead: text(s, Msg::FlyoutMsgFailedLead, &[("service", &svc)]),
            text: text_part,
            action: Some(LinkModel {
                label: s.template(Msg::FlyoutMsgRetry).to_owned(),
                action: CardAction::Retry,
            }),
        });
    }
    let Freshness::Stale(reason) = service.freshness else {
        return None;
    };
    if service.loading {
        return None;
    }
    let showing = last_reading(service)
        .map(|t| {
            format!(
                "{sep}{}",
                text(s, Msg::FlyoutMsgShowing, &[("time", &card_time(t, cx))])
            )
        })
        .unwrap_or_default();
    let next = service
        .status
        .next_try
        .map(|t| {
            format!(
                "{sep}{}",
                text(s, Msg::FlyoutMsgNextTry, &[("time", &card_time(t, cx))])
            )
        })
        .unwrap_or_default();
    match reason {
        StaleReason::SignIn => sign_in(),
        StaleReason::Offline | StaleReason::NoData | StaleReason::OldReading => {
            Some(MessageLine {
                kind: if reason == StaleReason::Offline {
                    MessageKind::Offline
                } else {
                    // The same look as NO_DATA; only the words differ.
                    MessageKind::NoData
                },
                glyph: SeverityGlyph::Stale,
                lead: s
                    .template(match reason {
                        StaleReason::Offline => Msg::FlyoutMsgOffline,
                        StaleReason::OldReading => Msg::FlyoutMsgOldReading,
                        _ => Msg::FlyoutMsgNoData,
                    })
                    .to_owned(),
                text: format!("{showing}{next}"),
                action: None,
            })
        }
        StaleReason::Paused => Some(MessageLine {
            kind: MessageKind::Paused,
            glyph: SeverityGlyph::Stale,
            lead: s.template(Msg::FlyoutMsgPaused).to_owned(),
            text: showing,
            action: Some(LinkModel {
                label: s.template(Msg::FlyoutMsgResume).to_owned(),
                action: CardAction::Resume,
            }),
        }),
    }
}

/// A window meter row (F§4.4).
fn meter_row(a: &Assessment, service: &ServiceEval, cx: &TextContext<'_>) -> MeterRowModel {
    let s = cx.strings;
    let reset = resets_at(a);
    let plain = |words: String| RightCluster {
        kind: RightKind::Plain,
        glyph: None,
        text: words,
    };
    let resets = |r| text(s, Msg::CardLineReset, &[("time", &card_time(r, cx))]);
    let reset_cluster = || match reset {
        Some(r) => RightCluster {
            kind: RightKind::Reset,
            glyph: None,
            text: resets(r),
        },
        None => plain(s.template(Msg::CardLineNoReset).to_owned()),
    };
    let say = |glyph: SeverityGlyph, words: String| RightCluster {
        kind: RightKind::Say,
        glyph: Some(glyph),
        text: words,
    };
    let w = window(a);
    let not_started =
        || w.is_some_and(|w| w.resets_at.is_none() && w.used <= 0.0 && w.starts_on_use);
    let right = match a.state {
        RowState::Ok | RowState::CriticalLevel | RowState::JustReset => reset_cluster(),
        RowState::Idle => plain(s.template(Msg::CardLineIdle).to_owned()),
        RowState::OkNoReset => plain(s.template(Msg::CardLineNoReset).to_owned()),
        RowState::OkBalance | RowState::Loading => plain(String::new()),
        RowState::Warning => match (t_out(a), reset) {
            (Some(t), _) => say(SeverityGlyph::Warning, runout_text(t, false, cx)),
            // Held by hysteresis without a run-out before the reset: ▲ + the reset; the trouble
            // line gives the projection.
            (None, Some(r)) => say(SeverityGlyph::Warning, resets(r)),
            (None, None) => reset_cluster(),
        },
        RowState::CriticalRunway => match t_out(a) {
            Some(t) => say(SeverityGlyph::Critical, runout_text(t, true, cx)),
            None => reset_cluster(),
        },
        // F§4.4: flyout times follow the card rule (`Back 4:30 PM`, `Back Mon 09:00`), not the
        // readout token.
        RowState::Exhausted => say(
            SeverityGlyph::Critical,
            match reset {
                Some(r) => text(s, Msg::Back, &[("time", &card_time(r, cx))]),
                None => s.template(Msg::Empty).to_owned(),
            },
        ),
        RowState::Stale(_) => match reset {
            Some(r) => plain(resets(r)),
            None if not_started() => plain(s.template(Msg::CardLineIdle).to_owned()),
            None => plain(s.template(Msg::CardLineNoReset).to_owned()),
        },
    };
    let line = match (a.state, w) {
        (RowState::JustReset, Some(w)) => match w.reset {
            ResetView::JustReset { previous } | ResetView::IdleAfterReset { previous } => {
                Some(text(
                    s,
                    Msg::CardLineJustReset,
                    &[("time", &card_time(previous, cx))],
                ))
            }
            ResetView::Live => None,
        },
        (RowState::Warning, _) => match (t_out(a), course_pct(a), reset) {
            (Some(t), _, Some(r)) => Some(text(
                s,
                Msg::FlyoutLineWarning,
                &[
                    ("duration", &before_reset(t, r, cx)),
                    ("time", &card_time(r, cx)),
                ],
            )),
            (None, Some(p), Some(r)) => Some(text(
                s,
                Msg::FlyoutLineWarningCourse,
                &[("p", &percent(p, cx.locale)), ("time", &card_time(r, cx))],
            )),
            _ => None,
        },
        (RowState::CriticalRunway, _) => match (t_out(a), reset) {
            (Some(t), Some(r)) => Some(text(
                s,
                Msg::FlyoutLineCritical,
                &[
                    ("time", &card_time(t, cx)),
                    ("duration", &before_reset(t, r, cx)),
                    ("reset", &card_time(r, cx)),
                ],
            )),
            _ => None,
        },
        (RowState::CriticalLevel, _) => Some(s.template(Msg::FlyoutLineLevel).to_owned()),
        (RowState::Exhausted, w) => reset.map(|r| {
            let duration = long_duration(shown_duration(cx.now, r), s);
            match w.and_then(|w| w.exhausted_since) {
                Some(since) => text(
                    s,
                    Msg::FlyoutLineExhausted,
                    &[("since", &card_time(since, cx)), ("duration", &duration)],
                ),
                None => text(
                    s,
                    Msg::FlyoutLineExhaustedNoSince,
                    &[("duration", &duration)],
                ),
            }
        }),
        _ => None,
    };
    let tone = match a.state {
        RowState::Warning => Tone::Caution,
        RowState::CriticalLevel | RowState::CriticalRunway | RowState::Exhausted => Tone::Critical,
        RowState::Stale(_) => Tone::Stale,
        _ => Tone::Calm,
    };
    let loading = a.state == RowState::Loading;
    MeterRowModel {
        key: a.key.clone(),
        name: a.names.name.clone(),
        value: if loading {
            s.template(Msg::NoReset).to_owned()
        } else {
            percent(pct(a), cx.locale)
        },
        value_dim: matches!(a.state, RowState::Stale(_)),
        right,
        line,
        tone,
        bar: bar_input(a, ColorRole::AccentFill),
        a11y: row_sentence(a, service, cx),
        pct: (!loading).then(|| pct(a)),
    }
}

/// The balance body (F§4.5).
fn balance_body(a: &Assessment, service: &ServiceEval, cx: &TextContext<'_>) -> BalanceBodyModel {
    let s = cx.strings;
    let Some(b) = balance(a) else {
        return BalanceBodyModel {
            key: a.key.clone(),
            amount: s.template(Msg::NoReset).to_owned(),
            unit: unit_word(&Unit::Credits, s),
            forecast: s.template(Msg::Loading).to_owned(),
            forecast_glyph: None,
            spent: String::new(),
            until: None,
            a11y: row_sentence(a, service, cx),
        };
    };
    let stats = &b.stats;
    let amount_text = amount(b.amount, cx.locale);
    let unit = unit_word(&b.unit, s);
    let pct_text = percent(b.pct, cx.locale);
    let burn = amount(stats.burn_per_day, cx.locale);
    let forecast = if stats.exhausted {
        s.template(Msg::Empty).to_owned()
    } else {
        balance_forecast_long(stats, s)
    };
    let forecast_glyph = match a.state {
        RowState::Warning => Some(SeverityGlyph::Warning),
        RowState::CriticalRunway | RowState::Exhausted => Some(SeverityGlyph::Critical),
        _ => None,
    };
    let mut spent = text(s, Msg::FlyoutBalanceSpent, &[("pct", &pct_text)]);
    let days = stats.days_left.and_then(|d| duration_from_hours(d * 24.0));
    if days.is_some() {
        spent.push_str(&text(s, Msg::FlyoutBalanceBurn, &[("burn", &burn)]));
    }
    let a11y = match days {
        Some(d) => text(
            s,
            Msg::FlyoutBalanceName,
            &[
                ("amount", &amount_text),
                ("unit", &unit),
                ("n", &b.pct.to_string()),
                ("duration", &spoken_duration(d, s)),
                ("burn", &burn),
            ],
        ),
        None => text(
            s,
            Msg::A11yBalanceNoForecast,
            &[
                ("service", &service.display_name()),
                ("amount", &amount_text),
                ("unit", &unit),
            ],
        ),
    };
    BalanceBodyModel {
        key: a.key.clone(),
        amount: amount_text,
        unit,
        forecast,
        forecast_glyph,
        spent,
        until: stats
            .t_out
            .filter(|_| days.is_some())
            .map(|t| text(s, Msg::FlyoutBalanceUntil, &[("date", &card_date(t, cx))])),
        a11y,
    }
}

/// A service card (F§4).
fn card(service: &ServiceEval, history: &dyn HistoryView, cx: &TextContext<'_>) -> CardModel {
    let s = cx.strings;
    let svc = service.display_name();
    let last = last_reading(service);
    let age = if service.loading {
        s.template(Msg::Loading).to_owned()
    } else {
        last.map(|t| age_long(t, cx)).unwrap_or_default()
    };
    let windows: Vec<&Assessment> = service
        .meters
        .iter()
        .filter(|a| !candidate(a).is_balance)
        .collect();
    let body = match service.meters.iter().find(|a| candidate(a).is_balance) {
        Some(b) if windows.is_empty() => CardBody::Balance(balance_body(b, service, cx)),
        _ => CardBody::Meters(windows.iter().map(|a| meter_row(a, service, cx)).collect()),
    };
    let a11y = if service.loading {
        svc.clone()
    } else {
        let age = last
            .map(|t| {
                text(
                    s,
                    Msg::A11yAgo,
                    &[("duration", &spoken_duration(shown_age(cx.now, t), s))],
                )
            })
            .unwrap_or_default();
        let mut name = match &service.account.plan {
            Some(plan) => text(
                s,
                Msg::FlyoutCardName,
                &[("service", &svc), ("plan", plan), ("age", &age)],
            ),
            None => text(
                s,
                Msg::FlyoutCardNameNoPlan,
                &[("service", &svc), ("age", &age)],
            ),
        };
        match service.freshness {
            Freshness::Stale(StaleReason::Offline) => {
                name.push_str(s.template(Msg::FlyoutCardOffline));
            }
            Freshness::Stale(StaleReason::SignIn) => {
                name.push_str(s.template(Msg::FlyoutCardSignIn));
            }
            _ => {}
        }
        name
    };
    CardModel {
        source: service.source.clone(),
        glyph: service.source.provider.glyph(),
        name: svc,
        plan: service.account.plan.clone(),
        age,
        message: message_line(service, cx),
        body,
        spark: spark_model(service, history, cx),
        a11y,
    }
}

/// A service card (F§4): border 1, radius 8, padding 12 → inner 310; header 20
/// (`[glyph 16] 8 [name] 8 [plan badge] [flex] [age] 8 [sparkline 55 × 16]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardModel {
    /// The service and account.
    pub source: SourceKey,
    /// Provider glyph, 16 px.
    pub glyph: ProviderGlyph,
    /// `Claude` (M2: `Claude · Work`), Text 14/20 semibold.
    pub name: String,
    /// Plan badge text; `None` = badge omitted.
    pub plan: Option<String>,
    /// `Just now`, `1 min ago`, …; LOADING `Reading…` (F§4.2).
    pub age: String,
    /// Message line (F§4.3).
    pub message: Option<MessageLine>,
    /// Meters or balance.
    pub body: CardBody,
    /// Sparkline.
    pub spark: SparkModel,
    /// Accessible label: `Claude, Max plan, updated 1 minute ago` (+ `, offline` /
    /// `, sign-in expired`), role groupbox (F§10).
    pub a11y: String,
}

/// The footer's left part (F§5): `Updates paused · Resume` (or `Update available · Install`,
/// M5), 12/16 TextSecondary with an accent hyperlink.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FooterNote {
    /// `Updates paused`.
    pub text: String,
    /// `Resume`.
    pub link: LinkModel,
}

/// Everything the flyout draws (F§1–F§10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlyoutModel {
    /// `Lastdrop` (Text 14/20 semibold).
    pub title: String,
    /// Header verdict; also the window's accessible description (F§10).
    pub verdict: Verdict,
    /// A refresh is running: the refresh glyph becomes a 16 px indeterminate progress ring and
    /// the button is disabled (F§3).
    pub refreshing: bool,
    /// `Refresh all services` (accessible name) and `Refresh (F5)` (tooltip).
    pub refresh_name: String,
    /// The refresh button's tooltip.
    pub refresh_tooltip: String,
    /// Cards in the user's service order (F§2.2).
    pub cards: Vec<CardModel>,
    /// `Settings` (footer button label).
    pub settings_label: String,
    /// Footer left part.
    pub footer_note: Option<FooterNote>,
    /// Opened by a click while a service needs sign-in (R§10): that card is scrolled into view
    /// and focused, without a focus rect (F§9).
    pub focus_card: Option<usize>,
    /// Announce once (a meter's severity rose while open, F§10).
    pub announce: Option<String>,
}

/// Builds the flyout model (F§3–F§10) from an evaluation; `history` feeds the sparklines and
/// `refreshing` the refresh button.
pub fn build_flyout(
    eval: &Evaluation,
    history: &dyn HistoryView,
    refreshing: bool,
    cx: &BuildContext<'_>,
) -> FlyoutModel {
    let t = &cx.text;
    let s = t.strings;
    let verdict = verdict(eval, t);
    let cards: Vec<CardModel> = eval
        .services
        .iter()
        .map(|service| card(service, history, t))
        .collect();
    let focus_card = cards.iter().position(|c| {
        c.message
            .as_ref()
            .is_some_and(|m| matches!(m.kind, MessageKind::SignIn | MessageKind::ApiKey))
    });
    let footer_note = cx.settings.pause.map(|_| FooterNote {
        text: s.template(Msg::FlyoutFooterPaused).to_owned(),
        link: LinkModel {
            label: s.template(Msg::FlyoutFooterResume).to_owned(),
            action: CardAction::Resume,
        },
    });
    let rose = eval
        .changes
        .iter()
        .any(|c| matches!(c, LiveChange::SeverityRose(_)));
    FlyoutModel {
        title: s.template(Msg::FlyoutTitle).to_owned(),
        announce: rose.then(|| verdict.text.clone()),
        verdict,
        refreshing,
        refresh_name: s.template(Msg::FlyoutRefreshName).to_owned(),
        refresh_tooltip: s.template(Msg::FlyoutRefreshTooltip).to_owned(),
        cards,
        settings_label: s.template(Msg::FlyoutSettings).to_owned(),
        footer_note,
        focus_card,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountKey, ProviderId};
    use crate::stats::Sample;
    use jiff::tz;
    use std::collections::BTreeMap;

    fn zone() -> TimeZone {
        TimeZone::fixed(tz::offset(2))
    }

    fn at(day: i8, hour: i8) -> Timestamp {
        jiff::civil::date(2026, 10, day)
            .at(hour, 0, 0, 0)
            .to_zoned(zone())
            .expect("valid")
            .timestamp()
    }

    fn spark(values: [Option<f64>; SPARK_DAYS], floor: f64) -> SparkModel {
        SparkModel {
            values,
            floor,
            dim: false,
            a11y: String::new(),
        }
    }

    #[test]
    fn geometry_fits_the_box_at_every_scale() {
        for (s, box_w, col, hair, x0) in [
            (1.0, 55, 3, 1, 0),
            (1.25, 69, 4, 1, 0),
            (1.5, 83, 5, 1, 0),
            (1.75, 96, 5, 1, 13),
            (2.0, 110, 6, 2, 0),
        ] {
            let g = spark_geometry(&spark([None; SPARK_DAYS], 10.0), s);
            assert_eq!(
                (g.box_w, g.col_w, g.hair, g.baseline.x0),
                (box_w, col, hair, x0)
            );
            assert_eq!(g.baseline.x1, box_w, "right-aligned at s = {s}");
            assert!(g.columns.is_empty());
        }
    }

    #[test]
    fn columns_scale_to_the_floor_and_mark_today() {
        let mut values = [None; SPARK_DAYS];
        values[0] = Some(5.0);
        values[1] = Some(0.0);
        values[12] = Some(0.01);
        values[13] = Some(20.0);
        let g = spark_geometry(&spark(values, 10.0), 1.0);
        // max = 20: 5 → round(5/20 × 15) = 4; 0.01 → at least one hair; 20 → 15.
        assert_eq!(
            g.columns,
            [
                SparkColumn {
                    x: 0,
                    h: 4,
                    today: false
                },
                SparkColumn {
                    x: 48,
                    h: 1,
                    today: false
                },
                SparkColumn {
                    x: 52,
                    h: 15,
                    today: true
                },
            ]
        );
        // A quiet fortnight stays low under the floor of 10 points.
        let mut quiet = [None; SPARK_DAYS];
        quiet[13] = Some(2.0);
        let g = spark_geometry(&spark(quiet, 10.0), 1.0);
        assert_eq!(g.columns[0].h, 3);
    }

    #[test]
    fn values_count_increases_per_local_day_and_ignore_resets() {
        let key = MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::new(MeterId::WEEKLY),
        };
        let r1 = Some(at(12, 9));
        let r2 = Some(at(19, 9));
        let mut history = BTreeMap::new();
        history.insert(
            key.clone(),
            vec![
                Sample::window(at(8, 10), 0.10, r1),
                Sample::window(at(9, 10), 0.15, r1),
                Sample::window(at(9, 20), 0.20, r1),
                Sample::window(at(10, 8), 0.18, r1), // correction: ignored
                Sample::window(at(10, 9), 0.25, r1),
                // Reset: the drop and the jump to a new instance count nothing.
                Sample::window(at(10, 11), 0.02, r2),
                Sample::window(at(10, 12), 0.05, r2),
            ],
        );
        let now = at(10, 12);
        let v = spark_values(&history, &key, SparkKind::Window, now, &zone());
        // Today: +7 and +3 points; the drop at the reset and the correction count nothing.
        assert_eq!(v[13].map(f64::round), Some(10.0));
        assert_eq!(v[12].map(f64::round), Some(10.0));
        assert_eq!(v[11], Some(0.0));
        assert_eq!(v[10], None);

        let bal_key = MeterKey {
            meter: MeterId::new(MeterId::CREDITS),
            provider: ProviderId::Meshy,
            ..key
        };
        let mut history = BTreeMap::new();
        history.insert(
            bal_key.clone(),
            vec![
                Sample::balance(at(9, 10), 1000.0),
                Sample::balance(at(10, 9), 900.0),
                Sample::balance(at(10, 10), 2000.0), // top-up: ignored
                Sample::balance(at(10, 11), 1950.0),
            ],
        );
        let v = spark_values(&history, &bal_key, SparkKind::Balance, now, &zone());
        assert_eq!(v[13], Some(150.0));
        assert_eq!(v[12], Some(0.0));
    }
}
