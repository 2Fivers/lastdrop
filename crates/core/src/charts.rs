//! The usage history window's charts (docs/design/charts-spec.md) as a model: each meter over
//! time, use per day and the hours of the week with the most use, from Lastdrop's own history.
//! All text and geometry inputs are made here; Slint only draws them. Pure: no I/O, no clock of
//! its own.
//!
//! Chart geometry is in shares of the chart's width and height (0 to 1), so Slint draws it at any
//! size with plain rectangles.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};

use crate::flyout::{SparkKind, daily_values};
use crate::grammar::LocaleFormat;
use crate::grammar::a11y::unit_word;
use crate::grammar::meters::localized;
use crate::grammar::numbers::{amount, percent, share_pct};
use crate::grammar::strings::{Msg, PluralMsg, Strings, capitalise, count, text};
use crate::grammar::time::{day_month, hour_minute, weekday_abbrev, weekday_full};
use crate::model::{MeterId, MeterKey, MeterNames, Unit, WindowLen};
use crate::stats::{HistoryView, Sample, instance_of, same_window_instance};
use crate::util::{add, sub};

/// Buckets across a chart (charts-spec §3): 28 minutes each over 7 days, 2 hours over 30, 6 hours
/// over 90.
pub const BUCKETS: usize = 360;

/// How long a reading stands for the buckets after it (§3): longer without one is a gap.
pub const CARRY_LIMIT: SignedDuration = SignedDuration::from_hours(3);

/// The longest time between two readings whose increase still counts for the active hours (§3).
pub const SPREAD_LIMIT: SignedDuration = SignedDuration::from_hours(2);

/// Shades of a busy heatmap cell (§2): 1 to this; 0 is a cell without use.
pub const HEAT_LEVELS: u8 = 4;

/// The window's period (§2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Range {
    /// The last 7 days.
    Week,
    /// The last 30 days (the default).
    #[default]
    Month,
    /// The last 90 days: all of the history.
    Quarter,
}

impl Range {
    /// Every range, in the combo box's order.
    pub const ALL: [Range; 3] = [Range::Week, Range::Month, Range::Quarter];

    /// Its local days, today included.
    pub fn days(self) -> usize {
        match self {
            Range::Week => 7,
            Range::Month => 30,
            Range::Quarter => 90,
        }
    }

    /// Days between the time axis's date labels (each day's weekday for a week).
    fn label_every(self) -> usize {
        match self {
            Range::Week => 1,
            Range::Month => 7,
            Range::Quarter => 14,
        }
    }

    /// `Last 30 days`.
    pub fn label(self, strings: &dyn Strings) -> String {
        text(
            strings,
            Msg::HistoryRangeDays,
            &[("n", &self.days().to_string())],
        )
    }
}

/// The local days of a range and the instants of their edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    /// The first local day.
    pub first_day: Date,
    /// How many days, today the last.
    pub days: usize,
    /// The start of the first day.
    pub from: Timestamp,
    /// The end of today (the start of tomorrow).
    pub to: Timestamp,
}

impl Period {
    /// The `range.days()` local days ending today.
    pub fn of(range: Range, now: Timestamp, tz: &TimeZone) -> Period {
        let days = range.days();
        let today = now.to_zoned(tz.clone()).date();
        let back = i64::try_from(days - 1).unwrap_or(0);
        let first_day = today.checked_sub(Span::new().days(back)).unwrap_or(today);
        let day = SignedDuration::from_hours(24);
        let from = start_of(first_day, tz).unwrap_or_else(|| sub(now, day * (back as i32 + 1)));
        let to = today
            .tomorrow()
            .ok()
            .and_then(|d| start_of(d, tz))
            .unwrap_or_else(|| add(now, day));
        Period {
            first_day,
            days,
            from,
            to,
        }
    }

    /// Where `t` is across the period, 0 to 1.
    pub fn fraction(&self, t: Timestamp) -> f64 {
        let total = self.to.duration_since(self.from).as_secs_f64();
        if total <= 0.0 {
            return 0.0;
        }
        (t.duration_since(self.from).as_secs_f64() / total).clamp(0.0, 1.0)
    }

    /// The start of bucket `i` (0 to [`BUCKETS`]; `BUCKETS` is the period's end).
    fn bucket_start(&self, i: usize) -> Timestamp {
        let total = i64::try_from(self.to.duration_since(self.from).as_nanos()).unwrap_or(0);
        let (i, n) = (i64::try_from(i).unwrap_or(0), BUCKETS as i64);
        add(
            self.from,
            SignedDuration::from_nanos(total / n * i + total % n * i / n),
        )
    }

    /// The start of local day `i` of the period.
    pub(crate) fn day_start(&self, i: usize, tz: &TimeZone) -> Option<Timestamp> {
        let date = self
            .first_day
            .checked_add(Span::new().days(i64::try_from(i).ok()?))
            .ok()?;
        start_of(date, tz)
    }
}

/// The first instant of `date` in `tz`.
fn start_of(date: Date, tz: &TimeZone) -> Option<Timestamp> {
    date.to_zoned(tz.clone()).ok().map(|z| z.timestamp())
}

/// One meter of the selected service, in the order it is charted.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartMeter {
    /// The meter.
    pub key: MeterKey,
    /// Its names as its provider gave them (localized here).
    pub names: MeterNames,
    /// A window meter or a balance.
    pub kind: SparkKind,
    /// A window meter's length, when its catalogue knows it (picks the history meter).
    pub window: Option<WindowLen>,
    /// A balance's unit, when it is known.
    pub unit: Option<Unit>,
}

/// What [`build_history`] works from.
pub struct HistoryInput<'a> {
    /// The service's meters, in order.
    pub meters: &'a [ChartMeter],
    /// Their samples.
    pub history: &'a dyn HistoryView,
    /// The period.
    pub range: Range,
    /// The clock.
    pub now: Timestamp,
    /// Regional format and time zone.
    pub locale: &'a LocaleFormat,
    /// The language.
    pub strings: &'a dyn Strings,
}

/// The window's charts for one service and period.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryModel {
    /// One chart per meter with readings in the period.
    pub charts: Vec<MeterChart>,
    /// The time axis's labels.
    pub ticks: Vec<Tick>,
    /// Use per day of the history meter.
    pub per_day: Option<DayChart>,
    /// Active hours of the history meter.
    pub hours: Option<HoursChart>,
    /// Why there is nothing to show (`history.empty`, `history.empty.range`).
    pub empty: Option<String>,
}

/// One meter over time.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterChart {
    /// The meter's name.
    pub title: String,
    /// Its latest value in the period (`42%`, `1,240 credits`).
    pub value: String,
    /// The top of the scale (`100%`, `2,000`).
    pub top: String,
    /// The values as steps: runs of buckets with the same value.
    pub bars: Vec<Bar>,
    /// The verticals between adjacent steps.
    pub risers: Vec<Riser>,
    /// The steps are filled down to the baseline (a window meter's used share); a balance is a
    /// line.
    pub filled: bool,
    /// What it shows, spoken.
    pub a11y: String,
}

/// A step of a chart: buckets `x0`–`x1` (shares of the width) at `height` (share of the height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// Left edge.
    pub x0: f32,
    /// Right edge.
    pub x1: f32,
    /// Height above the baseline.
    pub height: f32,
    /// A window meter was used up here (drawn in the critical colour).
    pub used_up: bool,
}

/// The vertical between two adjacent steps of different heights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Riser {
    /// Where, as a share of the width.
    pub x: f32,
    /// The lower step's height.
    pub low: f32,
    /// The higher step's height.
    pub high: f32,
}

/// A label of the time axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// Where its day starts, 0 to 1 across the chart.
    pub at: f32,
    /// `Mon`, `6 Oct`.
    pub label: String,
}

/// Use per day.
#[derive(Clone, Debug, PartialEq)]
pub struct DayChart {
    /// One column per day of the period, oldest first.
    pub columns: Vec<DayColumn>,
    /// The top of the scale (`20%`, `500 credits`).
    pub top: String,
    /// `Average 6% a day · highest 14% on Fri 10 Oct`.
    pub summary: String,
    /// What it shows, spoken.
    pub a11y: String,
}

/// One day's column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayColumn {
    /// Its height, 0 to 1 of the chart; 0 for a day without use or readings.
    pub height: f32,
    /// It is today.
    pub today: bool,
}

/// The hours of the week with the most use.
#[derive(Clone, Debug, PartialEq)]
pub struct HoursChart {
    /// 7 × 24 shades (0 to [`HEAT_LEVELS`]), Monday 00–01 first, row by row.
    pub levels: Vec<u8>,
    /// The weekdays' abbreviations, Monday first.
    pub rows: Vec<String>,
    /// The hour labels above the grid: `(hour, label)` at 0, 6, 12 and 18.
    pub columns: Vec<(u8, String)>,
    /// `Most use: Tuesday 14:00–15:00`.
    pub summary: String,
    /// What it shows, spoken.
    pub a11y: String,
}

/// The charts of `input.meters` over `input.range`.
pub fn build_history(input: &HistoryInput<'_>) -> HistoryModel {
    let strings = input.strings;
    let tz = &input.locale.tz;
    let period = Period::of(input.range, input.now, tz);
    // One day more, for the reading before the first one of the first day.
    let since = sub(period.from, SignedDuration::from_hours(24));
    let any_history = input
        .meters
        .iter()
        .any(|m| !input.history.samples(&m.key, Timestamp::MIN).is_empty());
    if !any_history {
        return HistoryModel {
            empty: Some(strings.template(Msg::HistoryEmpty).to_owned()),
            ..HistoryModel::default()
        };
    }
    let span = text(
        strings,
        Msg::A11yHistorySpan,
        &[("n", &period.days.to_string())],
    );
    let charts: Vec<MeterChart> = input
        .meters
        .iter()
        .filter_map(|m| {
            let samples = input.history.samples(&m.key, since);
            meter_chart(m, samples, &period, input, &span)
        })
        .collect();
    if charts.is_empty() {
        return HistoryModel {
            empty: Some(strings.template(Msg::HistoryEmptyRange).to_owned()),
            ..HistoryModel::default()
        };
    }
    let (per_day, hours) = match history_meter(input.meters) {
        Some(m) => {
            let samples = input.history.samples(&m.key, since);
            (
                day_chart(m, samples, &period, input, &span),
                hours_chart(m, samples, &period, input, &span),
            )
        }
        None => (None, None),
    };
    HistoryModel {
        charts,
        ticks: ticks(input.range, &period, input.locale),
        per_day,
        hours,
        empty: None,
    }
}

/// The meter use per day and the active hours are counted for (F§6's history meter): the
/// longest all-models window (`weekly` when there is one), else the first balance.
pub fn history_meter(meters: &[ChartMeter]) -> Option<&ChartMeter> {
    let windows = || meters.iter().filter(|m| m.kind == SparkKind::Window);
    windows()
        .find(|m| m.key.meter.0 == MeterId::WEEKLY)
        .or_else(|| windows().max_by_key(|m| nominal_length(m.window)))
        .or_else(|| meters.first())
}

/// A window's length for comparing windows (a calendar month as 30 days, unknown as none).
fn nominal_length(window: Option<WindowLen>) -> SignedDuration {
    match window {
        Some(WindowLen::Fixed(len)) => len,
        Some(WindowLen::CalendarMonth) => SignedDuration::from_hours(30 * 24),
        Some(WindowLen::Unknown) | None => SignedDuration::ZERO,
    }
}

/// The value of each bucket of the period (§3): a window meter's highest used share read in it,
/// a balance's last amount; a bucket without a reading carries the last one before it while it
/// is under [`CARRY_LIMIT`] old and its window has not reset. Buckets after `now` are empty.
pub fn buckets(
    samples: &[Sample],
    kind: SparkKind,
    period: &Period,
    now: Timestamp,
) -> Vec<Option<f64>> {
    let mut out = vec![None; BUCKETS];
    let mut next = samples.partition_point(|s| s.t < period.from);
    let mut last: Option<&Sample> = next
        .checked_sub(1)
        .and_then(|i| samples.get(i))
        .filter(|s| s.value.is_finite());
    for (i, slot) in out.iter_mut().enumerate() {
        let start = period.bucket_start(i);
        if start > now {
            break;
        }
        let end = period.bucket_start(i + 1);
        let mut value: Option<f64> = None;
        while let Some(s) = samples.get(next).filter(|s| s.t < end && s.t <= now) {
            if s.value.is_finite() {
                value = Some(match (kind, value) {
                    (SparkKind::Window, Some(v)) => v.max(s.value),
                    _ => s.value,
                });
                last = Some(s);
            }
            next += 1;
        }
        *slot = value.or_else(|| last.and_then(|s| carried(s, kind, start)));
    }
    out
}

/// The value `last` stands for at `at`, when it still does.
fn carried(last: &Sample, kind: SparkKind, at: Timestamp) -> Option<f64> {
    let age = at.duration_since(last.t);
    let reset = kind == SparkKind::Window && last.resets_at.is_some_and(|r| at >= r);
    (age < CARRY_LIMIT && !reset).then_some(last.value)
}

/// `values` on a scale of 0 to `top` as steps: each run of buckets with the same height (and,
/// with `used_up_from`, on the same side of it) is one [`Bar`]; adjacent bars of different
/// heights get a [`Riser`]. Empty buckets are gaps.
pub fn steps(
    values: &[Option<f64>],
    top: f64,
    used_up_from: Option<f64>,
) -> (Vec<Bar>, Vec<Riser>) {
    let n = values.len().max(1) as f64;
    let height = |v: f64| {
        let share = if top > 0.0 { v / top } else { 0.0 };
        share.clamp(0.0, 1.0) as f32
    };
    let mut bars: Vec<Bar> = Vec::new();
    let mut risers = Vec::new();
    let mut previous: Option<usize> = None;
    for (i, value) in values.iter().enumerate() {
        let Some(v) = *value else {
            previous = None;
            continue;
        };
        let h = height(v);
        let used_up = used_up_from.is_some_and(|from| v >= from);
        let x0 = (i as f64 / n) as f32;
        let x1 = ((i + 1) as f64 / n) as f32;
        if previous == Some(i.wrapping_sub(1))
            && let Some(last) = bars.last_mut()
        {
            if (last.height - h).abs() < 1e-6 && last.used_up == used_up {
                last.x1 = x1;
                previous = Some(i);
                continue;
            }
            if (last.height - h).abs() >= 1e-6 {
                risers.push(Riser {
                    x: x0,
                    low: last.height.min(h),
                    high: last.height.max(h),
                });
            }
        }
        bars.push(Bar {
            x0,
            x1,
            height: h,
            used_up,
        });
        previous = Some(i);
    }
    (bars, risers)
}

/// The first round value (1, 2 or 5 times a power of ten) at or above `v`; 1 for nothing.
pub fn nice_max(v: f64) -> f64 {
    if !(v.is_finite() && v > 0.0) {
        return 1.0;
    }
    let power = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|candidate| *candidate >= v * (1.0 - 1e-9))
        .unwrap_or(10.0 * power)
}

/// The use between consecutive readings in the period, spread evenly over the local hours
/// between them (§3), per weekday (Monday first) and hour. Increases between readings more than
/// [`SPREAD_LIMIT`] apart are left out.
pub fn active_hours(
    samples: &[Sample],
    kind: SparkKind,
    period: &Period,
    now: Timestamp,
    tz: &TimeZone,
) -> [[f64; 24]; 7] {
    let mut cells = [[0.0; 24]; 7];
    let mut previous: Option<(Sample, Option<Timestamp>)> = None;
    for &b in samples {
        let b_instance = instance_of(previous.and_then(|(_, i)| i), &b);
        if let Some((a, a_instance)) = previous
            && b.t > period.from
            && b.t <= now
        {
            let used = match kind {
                SparkKind::Window if same_window_instance(a_instance, b_instance) => {
                    (b.value - a.value).max(0.0) * 100.0
                }
                SparkKind::Window => 0.0,
                SparkKind::Balance => (a.value - b.value).max(0.0),
            };
            if used > 0.0 && used.is_finite() && b.t.duration_since(a.t) <= SPREAD_LIMIT {
                spread(&mut cells, a.t, b.t, used, period.from, tz);
            }
        }
        previous = Some((b, b_instance));
    }
    cells
}

/// Adds `used`, spread evenly over `a`–`b`, to the hours it covers from `from` on.
fn spread(
    cells: &mut [[f64; 24]; 7],
    a: Timestamp,
    b: Timestamp,
    used: f64,
    from: Timestamp,
    tz: &TimeZone,
) {
    let total = b.duration_since(a).as_secs_f64();
    if total <= 0.0 {
        add_to_hour(cells, b, used, tz);
        return;
    }
    let mut at = a.max(from);
    while at < b {
        let local = at.to_zoned(tz.clone());
        let into =
            SignedDuration::from_secs(i64::from(local.minute()) * 60 + i64::from(local.second()))
                + SignedDuration::from_nanos(i64::from(local.subsec_nanosecond()));
        let end = add(at, SignedDuration::from_hours(1) - into).min(b);
        if end <= at {
            break;
        }
        add_to_hour(
            cells,
            at,
            used * end.duration_since(at).as_secs_f64() / total,
            tz,
        );
        at = end;
    }
}

/// Adds `used` to the local hour of `t`.
fn add_to_hour(cells: &mut [[f64; 24]; 7], t: Timestamp, used: f64, tz: &TimeZone) {
    let local = t.to_zoned(tz.clone());
    let day = usize::try_from(local.weekday().to_monday_zero_offset()).unwrap_or(0) % 7;
    let hour = usize::try_from(local.hour()).unwrap_or(0) % 24;
    cells[day][hour] += used;
}

/// The shade of each cell: 0 without use, else 1 to [`HEAT_LEVELS`] by quarters of the busiest.
pub fn heat_levels(cells: &[[f64; 24]; 7]) -> Vec<u8> {
    let max = cells.iter().flatten().copied().fold(0.0, f64::max);
    cells
        .iter()
        .flatten()
        .map(|&v| {
            if v > 0.0 && max > 0.0 {
                let level = (v / max * f64::from(HEAT_LEVELS)).ceil();
                level.clamp(1.0, f64::from(HEAT_LEVELS)) as u8
            } else {
                0
            }
        })
        .collect()
}

/// The time axis's labels: each day's weekday for a week, else a date every
/// [`Range::label_every`] days, counted back from today.
fn ticks(range: Range, period: &Period, locale: &LocaleFormat) -> Vec<Tick> {
    let every = range.label_every();
    (0..period.days)
        .filter(|i| (period.days - 1 - i).is_multiple_of(every))
        .filter_map(|i| {
            let start = period.day_start(i, &locale.tz)?;
            let label = match range {
                Range::Week => weekday_abbrev(start, locale).to_owned(),
                Range::Month | Range::Quarter => day_month(start, locale),
            };
            Some(Tick {
                at: period.fraction(start) as f32,
                label,
            })
        })
        .collect()
}

/// A value of `meter` as shown: a share as a percent, an amount with its unit.
fn shown(meter: &ChartMeter, value: f64, input: &HistoryInput<'_>) -> String {
    match meter.kind {
        SparkKind::Window => percent(share_pct(value), input.locale),
        SparkKind::Balance => with_unit(amount(value, input.locale), meter, input.strings),
    }
}

/// `amount` followed by `meter`'s unit word, when it has one.
fn with_unit(amount: String, meter: &ChartMeter, strings: &dyn Strings) -> String {
    match &meter.unit {
        Some(unit) => format!("{amount} {}", unit_word(unit, strings)),
        None => amount,
    }
}

/// A full weekday and date for spoken text (`Friday 10 Oct`).
pub(crate) fn spoken_date(t: Timestamp, locale: &LocaleFormat) -> String {
    format!("{} {}", weekday_full(t, locale), day_month(t, locale))
}

/// A weekday and date as shown (`Fri 10 Oct`).
pub(crate) fn shown_date(t: Timestamp, locale: &LocaleFormat) -> String {
    format!("{} {}", weekday_abbrev(t, locale), day_month(t, locale))
}

/// The chart of one meter; `None` without a reading in the period.
fn meter_chart(
    meter: &ChartMeter,
    samples: &[Sample],
    period: &Period,
    input: &HistoryInput<'_>,
    span: &str,
) -> Option<MeterChart> {
    let in_period = || {
        samples
            .iter()
            .filter(|s| s.t >= period.from && s.t <= input.now && s.value.is_finite())
    };
    let latest = in_period().next_back()?;
    let highest = in_period().max_by(|a, b| a.value.total_cmp(&b.value))?;
    let values = buckets(samples, meter.kind, period, input.now);
    let strings = input.strings;
    let names = localized(meter.key.provider, &meter.key.meter, &meter.names, strings);
    let title = capitalise(&names.name);
    let date = spoken_date(highest.t, input.locale);
    Some(match meter.kind {
        SparkKind::Window => {
            let used_up: Vec<bool> = values.iter().map(|v| v.is_some_and(|v| v >= 1.0)).collect();
            let runs = used_up
                .iter()
                .zip(std::iter::once(&false).chain(used_up.iter()))
                .filter(|(now, before)| **now && !**before)
                .count();
            let (bars, risers) = steps(&values, 1.0, Some(1.0));
            let mut a11y = text(
                strings,
                Msg::A11yHistoryWindow,
                &[
                    ("meter", &title),
                    ("span", span),
                    ("n", &share_pct(latest.value).to_string()),
                    ("course", &share_pct(highest.value).to_string()),
                    ("date", &date),
                ],
            );
            if runs > 0 {
                a11y.push(' ');
                a11y.push_str(&count(
                    strings,
                    PluralMsg::A11yHistoryUsedUp,
                    i64::try_from(runs).unwrap_or(i64::MAX),
                ));
            }
            MeterChart {
                value: shown(meter, latest.value, input),
                top: percent(100, input.locale),
                bars,
                risers,
                filled: true,
                a11y,
                title,
            }
        }
        SparkKind::Balance => {
            let top = nice_max(highest.value);
            let (bars, risers) = steps(&values, top, None);
            let a11y = text(
                strings,
                Msg::A11yHistoryBalance,
                &[
                    ("meter", &title),
                    ("span", span),
                    ("amount", &shown(meter, latest.value, input)),
                    ("max", &shown(meter, highest.value, input)),
                    ("date", &date),
                ],
            );
            MeterChart {
                value: shown(meter, latest.value, input),
                top: amount(top, input.locale),
                bars,
                risers,
                filled: false,
                a11y,
                title,
            }
        }
    })
}

/// A per-day or per-hour use of `meter` as shown: percentage points as a percent, an amount
/// with its unit.
fn shown_use(meter: &ChartMeter, used: f64, input: &HistoryInput<'_>) -> String {
    match meter.kind {
        SparkKind::Window => percent(share_pct(used / 100.0), input.locale),
        SparkKind::Balance => with_unit(amount(used, input.locale), meter, input.strings),
    }
}

/// The same, spoken (`6 percent`).
fn spoken_use(meter: &ChartMeter, used: f64, input: &HistoryInput<'_>) -> String {
    match meter.kind {
        SparkKind::Window => text(
            input.strings,
            Msg::A11yHistoryPercent,
            &[("n", &share_pct(used / 100.0).to_string())],
        ),
        SparkKind::Balance => with_unit(amount(used, input.locale), meter, input.strings),
    }
}

/// Use per day of `meter`; `None` without a day with readings.
fn day_chart(
    meter: &ChartMeter,
    samples: &[Sample],
    period: &Period,
    input: &HistoryInput<'_>,
    span: &str,
) -> Option<DayChart> {
    let tz = &input.locale.tz;
    let values = daily_values(
        samples,
        meter.kind,
        period.first_day,
        period.days,
        period.from,
        input.now,
        tz,
    );
    let read: Vec<(usize, f64)> = values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (i, v)))
        .collect();
    let (best_day, best) = read
        .iter()
        .copied()
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))?;
    let average = read.iter().map(|(_, v)| v).sum::<f64>() / read.len() as f64;
    let floor = match meter.kind {
        SparkKind::Window => 1.0,
        SparkKind::Balance => 0.0,
    };
    let top = nice_max(best.max(floor));
    let last = period.days - 1;
    let columns = values
        .iter()
        .enumerate()
        .map(|(i, v)| DayColumn {
            height: (v.unwrap_or(0.0) / top).clamp(0.0, 1.0) as f32,
            today: i == last,
        })
        .collect();
    let best_start = period.day_start(best_day, tz).unwrap_or(period.from);
    let summary = text(
        input.strings,
        Msg::HistoryPerDaySummary,
        &[
            ("avg", &shown_use(meter, average, input)),
            ("max", &shown_use(meter, best, input)),
            ("date", &shown_date(best_start, input.locale)),
        ],
    );
    let a11y = text(
        input.strings,
        Msg::A11yHistoryPerDay,
        &[
            ("span", span),
            ("avg", &spoken_use(meter, average, input)),
            ("max", &spoken_use(meter, best, input)),
            ("date", &spoken_date(best_start, input.locale)),
        ],
    );
    Some(DayChart {
        columns,
        top: shown_use(meter, top, input),
        summary,
        a11y,
    })
}

/// The hour label of the grid and the summary: `14`, `2 PM` (12-hour clocks).
fn hour_label(hour: u8, locale: &LocaleFormat) -> String {
    let clock = &locale.clock;
    if clock.hour24 {
        if clock.leading_zero {
            format!("{hour:02}")
        } else {
            hour.to_string()
        }
    } else {
        let shown = match hour % 12 {
            0 => 12,
            h => h,
        };
        let designator = if hour < 12 { &clock.am } else { &clock.pm };
        if designator.is_empty() {
            shown.to_string()
        } else if clock.designator_first {
            format!("{designator} {shown}")
        } else {
            format!("{shown} {designator}")
        }
    }
}

/// Active hours of `meter`; `None` without any use counted.
fn hours_chart(
    meter: &ChartMeter,
    samples: &[Sample],
    period: &Period,
    input: &HistoryInput<'_>,
    span: &str,
) -> Option<HoursChart> {
    let locale = input.locale;
    let cells = active_hours(samples, meter.kind, period, input.now, &locale.tz);
    let (day, hour, _) = cells
        .iter()
        .enumerate()
        .flat_map(|(d, row)| row.iter().enumerate().map(move |(h, v)| (d, h, *v)))
        .filter(|(_, _, v)| *v > 0.0)
        .max_by(|a, b| a.2.total_cmp(&b.2).then((b.0, b.1).cmp(&(a.0, a.1))))?;
    let hour_i8 = i8::try_from(hour).unwrap_or(0);
    let time = hour_minute(hour_i8, 0, locale, true);
    let until = hour_minute((hour_i8 + 1) % 24, 0, locale, true);
    let weekday = &locale.weekdays_full[day];
    let summary = text(
        input.strings,
        Msg::HistoryHoursSummary,
        &[("day", weekday), ("time", &time), ("until", &until)],
    );
    let a11y = text(
        input.strings,
        Msg::A11yHistoryHours,
        &[
            ("span", span),
            ("day", weekday),
            ("time", &time),
            ("until", &until),
        ],
    );
    Some(HoursChart {
        levels: heat_levels(&cells),
        rows: locale.weekdays_abbrev.to_vec(),
        columns: [0u8, 6, 12, 18]
            .into_iter()
            .map(|h| (h, hour_label(h, locale)))
            .collect(),
        summary,
        a11y,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::grammar::English;
    use crate::model::{AccountKey, ProviderId};

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    fn utc_locale() -> LocaleFormat {
        LocaleFormat::en_gb(TimeZone::UTC)
    }

    fn key(meter: &str) -> MeterKey {
        MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::new(meter),
        }
    }

    fn week_period() -> Period {
        Period::of(Range::Week, ts("2026-10-16T12:00:00Z"), &TimeZone::UTC)
    }

    #[test]
    fn a_period_is_whole_local_days_ending_today() {
        let p = week_period();
        assert_eq!(p.first_day, jiff::civil::date(2026, 10, 10));
        assert_eq!(p.from, ts("2026-10-10T00:00:00Z"));
        assert_eq!(p.to, ts("2026-10-17T00:00:00Z"));
        assert_eq!(p.bucket_start(0), p.from);
        assert_eq!(p.bucket_start(BUCKETS), p.to);
        assert!((p.fraction(ts("2026-10-13T12:00:00Z")) - 0.5).abs() < 1e-9);
        let plus_two = TimeZone::fixed(jiff::tz::offset(2));
        let q = Period::of(Range::Month, ts("2026-10-16T12:00:00Z"), &plus_two);
        assert_eq!(q.from, ts("2026-09-16T22:00:00Z"), "local midnight");
        assert_eq!(q.to, ts("2026-10-16T22:00:00Z"));
    }

    #[test]
    fn buckets_take_the_peak_carry_for_a_while_and_stop_at_a_reset() {
        let p = week_period();
        let width = p.bucket_start(1).duration_since(p.bucket_start(0));
        assert_eq!(width, SignedDuration::from_mins(28));
        let reset = ts("2026-10-10T02:00:00Z");
        let samples = vec![
            Sample::window(ts("2026-10-10T00:05:00Z"), 0.30, Some(reset)),
            Sample::window(ts("2026-10-10T00:10:00Z"), 0.50, Some(reset)),
            Sample::window(ts("2026-10-10T00:20:00Z"), 0.40, Some(reset)),
        ];
        let v = buckets(&samples, SparkKind::Window, &p, ts("2026-10-16T12:00:00Z"));
        assert_eq!(v[0], Some(0.50), "the peak of the bucket");
        assert_eq!(v[1], Some(0.40), "carried");
        assert_eq!(v[4], Some(0.40), "01:52");
        assert_eq!(v[5], None, "02:20: the window reset at 02:00");

        let no_reset: Vec<Sample> = samples
            .iter()
            .map(|s| Sample::window(s.t, s.value, None))
            .collect();
        let v = buckets(&no_reset, SparkKind::Window, &p, ts("2026-10-16T12:00:00Z"));
        assert_eq!(v[7], Some(0.40), "03:16: 2 h 56 min after the last reading");
        assert_eq!(v[8], None, "03:44: over 3 hours");

        // A balance takes the last amount of a bucket; after `now` nothing is drawn.
        let balances = vec![
            Sample::balance(ts("2026-10-16T11:01:00Z"), 90.0),
            Sample::balance(ts("2026-10-16T11:05:00Z"), 80.0),
        ];
        let v = buckets(
            &balances,
            SparkKind::Balance,
            &p,
            ts("2026-10-16T12:00:00Z"),
        );
        let i = (0..BUCKETS)
            .find(|&i| p.bucket_start(i + 1) > ts("2026-10-16T11:05:00Z"))
            .expect("bucket");
        assert_eq!(v[i], Some(80.0));
        assert_eq!(v[BUCKETS - 1], None, "the evening has not come yet");
    }

    #[test]
    fn steps_merge_runs_rise_between_them_and_leave_gaps() {
        let values = [Some(0.5), Some(0.5), Some(1.0), None, Some(0.25)];
        let (bars, risers) = steps(&values, 1.0, Some(1.0));
        assert_eq!(
            bars,
            [
                Bar {
                    x0: 0.0,
                    x1: 0.4,
                    height: 0.5,
                    used_up: false
                },
                Bar {
                    x0: 0.4,
                    x1: 0.6,
                    height: 1.0,
                    used_up: true
                },
                Bar {
                    x0: 0.8,
                    x1: 1.0,
                    height: 0.25,
                    used_up: false
                },
            ]
        );
        assert_eq!(
            risers,
            [Riser {
                x: 0.4,
                low: 0.5,
                high: 1.0
            }],
            "none across the gap"
        );
        assert_eq!(steps(&[None, None], 1.0, None), (Vec::new(), Vec::new()));
        let (clamped, _) = steps(&[Some(3.0)], 2.0, None);
        assert_eq!(clamped[0].height, 1.0);
    }

    #[test]
    fn round_tops() {
        assert_eq!(nice_max(0.0), 1.0);
        assert_eq!(nice_max(f64::NAN), 1.0);
        assert_eq!(nice_max(7.0), 10.0);
        assert_eq!(nice_max(10.0), 10.0);
        assert_eq!(nice_max(11.0), 20.0);
        assert_eq!(nice_max(1240.0), 2000.0);
        assert_eq!(nice_max(0.03), 0.05);
    }

    #[test]
    fn active_hours_spread_increases_and_skip_long_gaps() {
        let p = week_period();
        let now = ts("2026-10-16T12:00:00Z");
        let samples = vec![
            // Friday 2026-10-16: 10:30 → 11:30, 10 points.
            Sample::window(ts("2026-10-16T10:30:00Z"), 0.10, None),
            Sample::window(ts("2026-10-16T11:30:00Z"), 0.20, None),
            // A gap of 3 hours: left out.
            Sample::window(ts("2026-10-15T01:00:00Z"), 0.10, None),
        ];
        let mut sorted = samples.clone();
        sorted.sort_by_key(|s| s.t);
        let cells = active_hours(&sorted, SparkKind::Window, &p, now, &TimeZone::UTC);
        assert!((cells[4][10] - 5.0).abs() < 1e-9, "{:?}", cells[4]);
        assert!((cells[4][11] - 5.0).abs() < 1e-9);
        let total: f64 = cells.iter().flatten().sum();
        assert!(
            (total - 10.0).abs() < 1e-9,
            "the drop and the long gap count nothing"
        );

        let levels = heat_levels(&cells);
        assert_eq!(levels.len(), 7 * 24);
        assert_eq!(levels[4 * 24 + 10], HEAT_LEVELS);
        assert_eq!(levels[0], 0);
    }

    #[test]
    fn the_history_meter_is_the_weekly_window_or_the_longest() {
        let window = |id: &str, hours: i64| ChartMeter {
            key: key(id),
            names: MeterNames::new(id, id, id),
            kind: SparkKind::Window,
            window: Some(WindowLen::Fixed(SignedDuration::from_hours(hours))),
            unit: None,
        };
        let meters = [window("five_hour", 5), window("weekly", 168)];
        assert_eq!(
            history_meter(&meters).map(|m| m.key.meter.0.as_str()),
            Some("weekly")
        );
        let meters = [window("a", 5), window("b", 24)];
        assert_eq!(
            history_meter(&meters).map(|m| m.key.meter.0.as_str()),
            Some("b")
        );
        assert_eq!(history_meter(&[]), None);
    }

    fn weekly_meter() -> ChartMeter {
        ChartMeter {
            key: key("weekly"),
            names: MeterNames::new("Wk", "weekly", "Weekly limit"),
            kind: SparkKind::Window,
            window: Some(WindowLen::Fixed(SignedDuration::from_hours(168))),
            unit: None,
        }
    }

    #[test]
    fn the_model_has_a_chart_per_meter_and_the_summaries() {
        let now = ts("2026-10-16T12:00:00Z");
        let reset = ts("2026-10-20T00:00:00Z");
        let mut history: BTreeMap<MeterKey, Vec<Sample>> = BTreeMap::new();
        let mut samples = Vec::new();
        // Thursday 10:00–12:00 from 20 % to 60 %, then a used-up stretch on Friday (read 21
        // hours later: it counts for the day, not for an hour).
        for (t, v) in [
            ("2026-10-15T10:00:00Z", 0.20),
            ("2026-10-15T11:00:00Z", 0.40),
            ("2026-10-15T12:00:00Z", 0.60),
            ("2026-10-16T09:00:00Z", 1.00),
            ("2026-10-16T10:00:00Z", 1.00),
        ] {
            samples.push(Sample::window(ts(t), v, Some(reset)));
        }
        history.insert(key("weekly"), samples);
        let meters = [weekly_meter()];
        let locale = utc_locale();
        let model = build_history(&HistoryInput {
            meters: &meters,
            history: &history,
            range: Range::Week,
            now,
            locale: &locale,
            strings: &English,
        });
        assert_eq!(model.empty, None);
        assert_eq!(model.charts.len(), 1);
        let chart = &model.charts[0];
        assert_eq!(chart.value, "100%");
        assert_eq!(chart.top, "100%");
        assert!(chart.filled);
        assert!(chart.bars.iter().any(|b| b.used_up));
        assert!(chart.bars.iter().any(|b| !b.used_up));
        assert!(chart.a11y.contains("now 100 percent"), "{}", chart.a11y);
        assert!(chart.a11y.ends_with("Used up 1 time."), "{}", chart.a11y);
        assert_eq!(model.ticks.len(), 7);
        assert_eq!(model.ticks[0].label, "Sat");
        assert_eq!(model.ticks[6].label, "Fri");

        let per_day = model.per_day.expect("per day");
        assert_eq!(per_day.columns.len(), 7);
        assert!(per_day.columns[6].today);
        // Thursday 40 points, Friday 40 (60 % → 100 %): the earlier day is named.
        assert_eq!(
            per_day.summary,
            "Average 40% a day · highest 40% on Thu 15 Oct"
        );
        let hours = model.hours.expect("hours");
        assert_eq!(hours.rows[0], "Mon");
        assert_eq!(hours.columns[2], (12, "12".to_owned()));
        assert_eq!(hours.summary, "Most use: Thursday 10:00–11:00");
    }

    #[test]
    fn nothing_recorded_says_so() {
        let locale = utc_locale();
        let meters = [weekly_meter()];
        let empty: BTreeMap<MeterKey, Vec<Sample>> = BTreeMap::new();
        let input = HistoryInput {
            meters: &meters,
            history: &empty,
            range: Range::Week,
            now: ts("2026-10-16T12:00:00Z"),
            locale: &locale,
            strings: &English,
        };
        assert_eq!(
            build_history(&input).empty.as_deref(),
            Some(English.template(Msg::HistoryEmpty))
        );
        let mut old = BTreeMap::new();
        old.insert(
            key("weekly"),
            vec![Sample::window(ts("2026-08-01T00:00:00Z"), 0.5, None)],
        );
        let input = HistoryInput {
            history: &old,
            ..input
        };
        assert_eq!(
            build_history(&input).empty.as_deref(),
            Some(English.template(Msg::HistoryEmptyRange))
        );
    }
}
