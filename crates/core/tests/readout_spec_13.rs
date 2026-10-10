//! The test vectors of readout-spec §13, as executable checks against `lastdrop-core`'s public API.
//!
//! The inputs and expected values are copied from the spec; where a vector needs more inputs than
//! the spec prints (a history, a status), the extra inputs are chosen so that they cannot change
//! the expected value, and the comment says so. Values the spec prints rounded (`p = 1.595`) are
//! compared at the precision printed.
//!
//! `now = Sat 2026-10-10 12:42` local, 24 h locale unless noted. "Local" is a fixed UTC+02:00
//! zone (Stockholm in October before the DST change), so the vectors do not depend on a tzdb.
//! The widths vector (`160 / 176 / 186 / 134 / 150`) needs DirectWrite and lives in
//! `crates/win/tests/readout_widths.rs`.

use jiff::tz::{self, TimeZone};
use jiff::{SignedDuration, Timestamp, civil};
use lastdrop_core::color::ColorRole;
use lastdrop_core::grammar::cells::{RowPosition, right_cell};
use lastdrop_core::grammar::numbers::{balance_duration, percent, share_pct};
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::time::{
    CountdownForm, age_token, card_date, countdown, long_duration, runout_text, shown_duration,
    time_token,
};
use lastdrop_core::grammar::{English, LocaleFormat, Msg, TextContext, TimeStyle};
use lastdrop_core::model::{
    AccountRef, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId, Snapshot, SourceKind,
    WindowLen,
};
use lastdrop_core::readout::bar::{BarInput, BarMode, Seg, bar_geometry, bar_input};
use lastdrop_core::settings::Settings;
use lastdrop_core::stats::{
    self, BalanceSeverityInput, CriticalCause, DisplayClass, EvalInput, EvalMemory, Evaluation,
    FailureKind, Freshness, NoHistory, RowState, Sample, ServiceInput, ServiceStatus, Severity,
    SeverityMemory, StaleReason, WindowSeverityInput,
};

// ---- helpers ------------------------------------------------------------------------------------

fn local_tz() -> TimeZone {
    TimeZone::fixed(tz::offset(2))
}

/// A local wall-clock time in the test zone.
fn at(month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
    at_s(month, day, hour, minute, 0, 0)
}

fn at_s(month: i8, day: i8, hour: i8, minute: i8, second: i8, millis: i32) -> Timestamp {
    civil::date(2026, month, day)
        .at(hour, minute, second, millis * 1_000_000)
        .to_zoned(local_tz())
        .expect("valid local time")
        .timestamp()
}

/// `now` of R§13: Sat 2026-10-10 12:42.
fn now() -> Timestamp {
    at(10, 10, 12, 42)
}

fn mins(m: i64) -> SignedDuration {
    SignedDuration::from_mins(m)
}

fn hours(h: i64) -> SignedDuration {
    SignedDuration::from_hours(h)
}

fn en_gb() -> LocaleFormat {
    LocaleFormat::en_gb(local_tz())
}

fn en_us() -> LocaleFormat {
    LocaleFormat::en_us(local_tz())
}

fn cx<'a>(now: Timestamp, locale: &'a LocaleFormat, style: TimeStyle) -> TextContext<'a> {
    TextContext {
        now,
        time_style: style,
        locale,
        strings: &English,
    }
}

fn names(short_label: &str, short_name: &str, name: &str) -> MeterNames {
    MeterNames {
        short_label: short_label.to_owned(),
        short_name: short_name.to_owned(),
        name: name.to_owned(),
    }
}

fn five_hour(used: f64, resets_at: Option<Timestamp>) -> Meter {
    Meter {
        id: MeterId::new(MeterId::FIVE_HOUR),
        names: names("5h", "5-hour", "5-hour session"),
        order: 0,
        used,
        limited: false,
        resets_at,
        window: WindowLen::Fixed(hours(5)),
        starts_on_use: true,
    }
}

fn weekly(used: f64, resets_at: Option<Timestamp>) -> Meter {
    Meter {
        id: MeterId::new(MeterId::WEEKLY),
        names: names("Wk", "weekly", "Weekly · all models"),
        order: 1,
        used,
        limited: false,
        resets_at,
        window: WindowLen::Fixed(hours(7 * 24)),
        starts_on_use: false,
    }
}

fn claude_snapshot(meters: Vec<Meter>, fetched_at: Timestamp) -> Snapshot {
    Snapshot {
        provider: ProviderId::Claude,
        account: AccountRef::primary(),
        source: SourceKind::ClaudeOAuthUsage,
        measured_at: fetched_at,
        fetched_at,
        meters,
        balances: Vec::new(),
        health: Health::Ok,
    }
}

fn key(meter: &str) -> MeterKey {
    MeterKey {
        provider: ProviderId::Claude,
        account: lastdrop_core::model::AccountKey::primary(),
        meter: MeterId::new(meter),
    }
}

/// Evaluates one fresh Claude snapshot (last success = its fetch time, 5-minute interval).
fn evaluate_one(snapshot: &Snapshot, now: Timestamp) -> Evaluation {
    let settings = Settings::default();
    let account = AccountRef::primary();
    let status = ServiceStatus {
        last_success: Some(snapshot.fetched_at),
        poll_interval: Some(mins(5)),
        ..ServiceStatus::default()
    };
    let services = [ServiceInput {
        provider: ProviderId::Claude,
        account: &account,
        snapshot: Some(snapshot),
        status: &status,
        fetching: false,
    }];
    let input = EvalInput {
        now,
        settings: &settings,
        services: &services,
        history: &NoHistory,
    };
    stats::evaluate(&input, &mut EvalMemory::default())
}

fn calm_bar(used: f64, elapsed: Option<f64>, mode: BarMode) -> BarInput {
    BarInput {
        used,
        elapsed,
        mode,
        dashed: false,
        fill: ColorRole::TextPrimary,
        tick: ColorRole::TextPrimary,
        track: ColorRole::Track,
    }
}

fn segs(list: &[(i32, i32)]) -> Vec<Seg> {
    list.iter().map(|&(x0, x1)| Seg { x0, x1 }).collect()
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

fn assert_near_time(actual: Timestamp, expected: Timestamp) {
    let diff = actual.duration_since(expected).abs();
    assert!(
        diff <= SignedDuration::from_secs(1),
        "{actual} is not within 1 s of {expected}"
    );
}

// ---- Elapsed share (R§3.2) ------------------------------------------------------------------------

mod elapsed_share {
    use super::*;

    #[test]
    fn five_hour_window_resetting_at_14_56() {
        assert_close(
            stats::elapsed_share(now(), at(10, 10, 14, 56), hours(5)),
            0.5533,
            5e-5,
        );
    }

    #[test]
    fn weekly_window_resetting_mon_09_00() {
        assert_close(
            stats::elapsed_share(now(), at(10, 12, 9, 0), hours(7 * 24)),
            0.7363,
            5e-5,
        );
    }

    #[test]
    fn weekly_window_resetting_fri_16_oct_14_26() {
        assert_close(
            stats::elapsed_share(now(), at(10, 16, 14, 26), hours(7 * 24)),
            0.1325,
            5e-5,
        );
    }
}

// ---- Display percent (R§3.3) ----------------------------------------------------------------------

mod display_percent {
    use super::*;

    #[test]
    fn table() {
        // (u, limited, pct, exhausted)
        let cases = [
            (0.0, false, 0, false),
            (0.004, false, 1, false),
            (0.4249, false, 42, false),
            (0.425, false, 43, false),
            (0.8949, false, 89, false),
            (0.895, false, 90, false),
            (0.996, false, 99, false),
            (1.0, false, 100, true),
            (0.97, true, 97, true),
        ];
        for (u, limited, pct, exhausted) in cases {
            let ex = stats::is_exhausted(u, limited);
            assert_eq!(ex, exhausted, "exhausted for u = {u}, limited = {limited}");
            assert_eq!(stats::display_pct(u, ex), pct, "pct for u = {u}");
        }
    }

    #[test]
    fn level_threshold_compares_the_shown_percent() {
        let input = |pct| WindowSeverityInput {
            pct,
            exhausted: false,
            valid: false,
            runs_out: false,
            projected: None,
            runway_min: None,
        };
        let not_critical = stats::window_severity(
            &input(stats::display_pct(0.8949, false)),
            SeverityMemory::default(),
        );
        assert_eq!(not_critical.severity, Severity::Ok);
        let critical = stats::window_severity(
            &input(stats::display_pct(0.895, false)),
            SeverityMemory::default(),
        );
        assert_eq!(critical.severity, Severity::Critical);
        assert_eq!(critical.cause, Some(CriticalCause::Level));
    }
}

// ---- Time tokens (R§5.2) --------------------------------------------------------------------------

mod time_tokens {
    use super::*;

    #[test]
    fn clock_12h_and_relative() {
        let gb = en_gb();
        let us = en_us();
        // (R, clock, 12-h locale, relative)
        let cases = [
            (at(10, 10, 14, 56), "14:56", "2:56", "2h"),
            (at(10, 11, 0, 30), "00:30", "12:30", "11h"),
            (at(10, 11, 1, 0), "Sun", "Sun", "12h"),
            (at(10, 12, 9, 0), "Mon", "Mon", "1d"),
            (at(10, 16, 14, 26), "Fri", "Fri", "6d"),
            (at(10, 17, 9, 0), "7d", "7d", "6d"),
            (at(11, 1, 0, 0), "22d", "22d", "21d"),
        ];
        for (r, clock, h12, relative) in cases {
            assert_eq!(
                time_token(r, &cx(now(), &gb, TimeStyle::Clock)),
                clock,
                "clock {r}"
            );
            assert_eq!(
                time_token(r, &cx(now(), &us, TimeStyle::Clock)),
                h12,
                "12-h {r}"
            );
            assert_eq!(
                time_token(r, &cx(now(), &gb, TimeStyle::Relative)),
                relative,
                "rel {r}"
            );
        }
    }

    #[test]
    fn today_when_12_hours_or_more_away_on_the_same_date() {
        let (gb, us) = (en_gb(), en_us());
        let early = at(10, 10, 0, 30);
        let r = at(10, 10, 18, 0);
        assert_eq!(time_token(r, &cx(early, &gb, TimeStyle::Clock)), "Today");
        assert_eq!(time_token(r, &cx(early, &us, TimeStyle::Clock)), "Today");
        assert_eq!(time_token(r, &cx(early, &gb, TimeStyle::Relative)), "17h");
    }
}

// ---- Rounding (R§5.0) -----------------------------------------------------------------------------

mod rounding {
    use super::*;

    #[test]
    fn reset_at_14_59_59_94_shows_15_00_and_2_h_18_min() {
        let gb = en_gb();
        let now = at_s(10, 10, 12, 42, 30, 0);
        let r = at_s(10, 10, 14, 59, 59, 940);
        let c = cx(now, &gb, TimeStyle::Clock);
        assert_eq!(time_token(r, &c), "15:00");
        let duration = long_duration(shown_duration(now, r), &English);
        assert_eq!(
            text(&English, Msg::CardLineIn, &[("duration", &duration)]),
            "in 2 h 18 min"
        );
    }

    #[test]
    fn durations_from_12_42() {
        let cases = [
            (at(10, 10, 14, 56), "2 h 14 min"),
            (at(10, 12, 9, 0), "1 d 20 h"),
            (at(11, 1, 0, 0), "21 d"),
            (at(10, 10, 13, 27), "45 min"),
        ];
        for (t, want) in cases {
            assert_eq!(
                long_duration(shown_duration(now(), t), &English),
                want,
                "{t}"
            );
        }
    }
}

// ---- Countdown (R§5.3) ----------------------------------------------------------------------------

mod countdown_vectors {
    use super::*;

    fn now() -> Timestamp {
        at(10, 10, 14, 50)
    }

    fn left(t_out: Timestamp, critical: bool) -> String {
        let gb = en_gb();
        runout_text(t_out, critical, &cx(now(), &gb, TimeStyle::Clock))
    }

    #[test]
    fn minutes() {
        assert_eq!(left(now() + mins(12), false), "12 min left");
        // 15:01:30 rounds to 15:02 → N 12
        assert_eq!(left(at_s(10, 10, 15, 1, 30, 0), false), "12 min left");
        // +20 s → N never shows 0
        assert_eq!(
            left(now() + SignedDuration::from_secs(20), false),
            "1 min left"
        );
        // 15:49:12 → 15:49 → N 59
        assert_eq!(left(at_s(10, 10, 15, 49, 12, 0), false), "59 min left");
    }

    #[test]
    fn sixty_minutes_is_out_for_warning_and_hours_for_critical() {
        // +59.6 min → 15:49:36 → 15:50 → N 60
        let t_out = at_s(10, 10, 15, 49, 36, 0);
        assert_eq!(stats::runway_minutes(now(), t_out), 60);
        assert_eq!(left(t_out, false), "Out 15:50");
        assert_eq!(left(t_out, true), "1 h left");
    }

    #[test]
    fn hours_and_days_floor() {
        assert_eq!(left(now() + hours(6) + mins(10), true), "6 h left");
        assert_eq!(left(now() + hours(47) + mins(59), true), "47 h left");
        assert_eq!(left(now() + hours(48), true), "2 d left");
    }

    #[test]
    fn compact_form() {
        assert_eq!(countdown(12, CountdownForm::Compact, &English), "12 min");
        assert_eq!(
            countdown(6 * 60 + 10, CountdownForm::Compact, &English),
            "6 h"
        );
        assert_eq!(countdown(12, CountdownForm::Left, &English), "12 min left");
    }
}

// ---- Age (R§5.4) ----------------------------------------------------------------------------------

mod age {
    use super::*;

    #[test]
    fn tokens() {
        let gb = en_gb();
        let now = at(10, 10, 14, 50);
        let c = cx(now, &gb, TimeStyle::Clock);
        assert_eq!(age_token(now - mins(15), &c), "15m ago");
        assert_eq!(age_token(now - mins(59), &c), "59m ago");
        // 59.9 min: 13:50:06 → 13:50 → 60 min
        assert_eq!(age_token(at_s(10, 10, 13, 50, 6, 0), &c), "1h ago");
        assert_eq!(age_token(now - SignedDuration::from_secs(12), &c), "1m ago");
        assert_eq!(age_token(now - hours(47), &c), "47h ago");
        assert_eq!(age_token(now - hours(49), &c), "2d ago");
    }
}

// ---- No reset time (R§3.2, R§3.9) -----------------------------------------------------------------

mod no_reset_time {
    use super::*;

    #[test]
    fn claude_five_hour_after_its_reset_is_idle() {
        let snap = claude_snapshot(vec![five_hour(0.0, None)], now() - mins(1));
        let eval = evaluate_one(&snap, now());
        let (service, a) = eval.assessment(&key(MeterId::FIVE_HOUR)).expect("assessed");
        assert_eq!(a.state, RowState::Idle);
        assert_eq!(a.class, DisplayClass::Ok);
        assert_eq!(a.pressure, 0.0);
        let gb = en_gb();
        let pos = RowPosition {
            index_in_service: 0,
            rows_in_service: 1,
        };
        let cell = right_cell(a, service, pos, &cx(now(), &gb, TimeStyle::Clock));
        assert_eq!(cell.texts(), ["0%", "–"]);
        let geom = bar_geometry(&bar_input(a, ColorRole::TextPrimary), 40.0, 16.0, 1.0);
        assert!(geom.fill.is_empty(), "track only");
        assert_eq!(geom.tick, None);
    }

    #[test]
    fn weekly_after_its_reset_without_a_new_reading_is_just_reset() {
        let now = at(10, 12, 9, 1);
        // Read at 08:58, before the 09:00 reset, so the service is fresh.
        let snap = claude_snapshot(
            vec![weekly(0.61, Some(at(10, 12, 9, 0)))],
            at(10, 12, 8, 58),
        );
        let eval = evaluate_one(&snap, now);
        let (service, a) = eval.assessment(&key(MeterId::WEEKLY)).expect("assessed");
        assert_eq!(a.state, RowState::JustReset);
        let gb = en_gb();
        let pos = RowPosition {
            index_in_service: 0,
            rows_in_service: 1,
        };
        let cell = right_cell(a, service, pos, &cx(now, &gb, TimeStyle::Clock));
        assert_eq!(cell.texts(), ["0%", "7d"]); // "↺" is a glyph run before "7d"
        let geom = bar_geometry(&bar_input(a, ColorRole::TextPrimary), 40.0, 16.0, 1.0);
        assert_eq!(geom.tick, Some(0));

        let w = stats::effective_window(&weekly(0.61, Some(at(10, 12, 9, 0))), now);
        assert_eq!(w.used, 0.0);
        assert_eq!(w.resets_at, Some(at(10, 19, 9, 0)));
    }

    #[test]
    fn reset_time_none_with_forty_percent() {
        let snap = claude_snapshot(vec![weekly(0.40, None)], now() - mins(1));
        let eval = evaluate_one(&snap, now());
        let (service, a) = eval.assessment(&key(MeterId::WEEKLY)).expect("assessed");
        assert_eq!(a.state, RowState::OkNoReset);
        let gb = en_gb();
        let pos = RowPosition {
            index_in_service: 0,
            rows_in_service: 1,
        };
        let cell = right_cell(a, service, pos, &cx(now(), &gb, TimeStyle::Clock));
        assert_eq!(cell.texts(), ["40%", "–"]);
        let geom = bar_geometry(&bar_input(a, ColorRole::TextPrimary), 40.0, 16.0, 1.0);
        assert_eq!(geom.tick, None);
        match &a.meter {
            stats::AssessedMeter::Window(w) => assert!(w.forecast.is_none()),
            other => panic!("expected a window meter, got {other:?}"),
        }
    }

    #[test]
    fn unknown_window_length_has_no_tick_a_one_hour_span_and_a_guard_from_the_first_reading() {
        let r = at(10, 10, 18, 0);
        assert_eq!(stats::elapsed(Some(r), None, now()), None);
        assert_eq!(stats::trailing_span(None), hours(1));
        // First reading of the instance at 12:00: at 12:10 the fresh-window guard still holds.
        let early = [
            Sample::window(at(10, 10, 12, 0), 0.1, Some(r)),
            Sample::window(at(10, 10, 12, 10), 0.2, Some(r)),
        ];
        assert!(
            !stats::forecast(&early, Some(r), None)
                .expect("forecast")
                .valid
        );
        let later = [
            Sample::window(at(10, 10, 12, 0), 0.1, Some(r)),
            Sample::window(at(10, 10, 12, 20), 0.3, Some(r)),
        ];
        assert!(
            stats::forecast(&later, Some(r), None)
                .expect("forecast")
                .valid
        );
    }
}

// ---- Pace bar geometry (R§6.6) --------------------------------------------------------------------

mod pace_bar {
    use super::*;

    #[test]
    fn u_042_e_05533_at_four_scales() {
        let input = calm_bar(0.42, Some(0.5533), BarMode::Normal);
        // (s, Lpx, hair, fillH, tickH, f, k, track, fill)
        #[allow(clippy::type_complexity, reason = "a literal table")]
        let rows: [(
            f64,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            &[(i32, i32)],
            &[(i32, i32)],
        ); 4] = [
            (
                1.0,
                40,
                1,
                3,
                9,
                17,
                22,
                &[(17, 21), (22, 23), (24, 40)],
                &[(0, 17)],
            ),
            (
                1.25,
                50,
                1,
                3,
                11,
                21,
                28,
                &[(21, 27), (28, 29), (30, 50)],
                &[(0, 21)],
            ),
            (
                1.5,
                60,
                1,
                5,
                13,
                25,
                33,
                &[(25, 32), (33, 34), (35, 60)],
                &[(0, 25)],
            ),
            (
                2.0,
                80,
                2,
                6,
                18,
                34,
                44,
                &[(34, 42), (44, 46), (48, 80)],
                &[(0, 34)],
            ),
        ];
        for (s, len, hair, fill_h, tick_h, f, k, track, fill) in rows {
            let g = bar_geometry(&input, 40.0, 16.0, s);
            assert_eq!(
                (g.len, g.hair, g.fill_h, g.tick_h),
                (len, hair, fill_h, tick_h),
                "s = {s}"
            );
            assert_eq!((g.fill_end, g.tick), (f, Some(k)), "s = {s}");
            assert_eq!(g.track, segs(track), "track at s = {s}");
            assert_eq!(g.fill, segs(fill), "fill at s = {s}");
        }
    }

    #[test]
    fn ahead_of_pace_runs_visibly_past_the_tick() {
        let g = bar_geometry(
            &calm_bar(0.68, Some(0.63), BarMode::Normal),
            40.0,
            16.0,
            1.0,
        );
        assert_eq!((g.fill_end, g.tick), (28, Some(25)));
        assert_eq!(g.fill, segs(&[(0, 24), (25, 26), (27, 28)]));
    }

    #[test]
    fn on_pace() {
        let g = bar_geometry(
            &calm_bar(0.55, Some(0.5533), BarMode::Normal),
            40.0,
            16.0,
            1.0,
        );
        assert_eq!((g.fill_end, g.tick), (22, Some(22)));
        assert_eq!(g.fill, segs(&[(0, 21)]));
    }

    #[test]
    fn nothing_used_at_the_start_of_a_window() {
        let g = bar_geometry(
            &calm_bar(0.0, Some(0.003), BarMode::Normal),
            40.0,
            16.0,
            1.0,
        );
        assert!(g.fill.is_empty());
        assert_eq!(g.tick, Some(0));
        assert_eq!(g.track, segs(&[(0, 1), (2, 40)]));
    }

    #[test]
    fn exhausted_fills_the_whole_length_minus_the_gaps() {
        let g = bar_geometry(
            &calm_bar(1.0, Some(0.70), BarMode::Exhausted),
            40.0,
            16.0,
            1.0,
        );
        assert_eq!(g.fill, segs(&[(0, 27), (28, 29), (30, 40)]));
        assert!(g.track.is_empty());
    }

    #[test]
    fn something_used_is_at_least_one_pixel() {
        let g = bar_geometry(
            &calm_bar(0.004, Some(0.30), BarMode::Normal),
            40.0,
            16.0,
            1.0,
        );
        assert_eq!(g.fill_end, 1);
    }

    #[test]
    fn tick_at_the_end_is_clamped() {
        let g = bar_geometry(&calm_bar(0.5, Some(1.0), BarMode::Normal), 40.0, 16.0, 1.0);
        assert_eq!(g.tick, Some(39));
    }
}

// ---- Forecast (R§3.5, R§3.7, R§5.3) ---------------------------------------------------------------

mod forecast {
    use super::*;

    fn r() -> Timestamp {
        at(10, 10, 16, 30)
    }

    fn readings(list: &[(Timestamp, f64)], reset: Timestamp) -> Vec<Sample> {
        list.iter()
            .map(|&(t, u)| Sample::window(t, u, Some(reset)))
            .collect()
    }

    fn severity(pct: u8, f: &stats::Forecast, now: Timestamp) -> stats::SeverityOutcome {
        let input = WindowSeverityInput {
            pct,
            exhausted: false,
            valid: f.valid,
            runs_out: f.runs_out,
            projected: Some(f.projected),
            runway_min: f.t_out.map(|t| stats::runway_minutes(now, t)),
        };
        stats::window_severity(&input, SeverityMemory::default())
    }

    #[test]
    fn warning_out_15_19() {
        let tn = at(10, 10, 13, 45);
        let f = stats::forecast(
            &readings(&[(at(10, 10, 13, 15), 0.71), (tn, 0.78)], r()),
            Some(r()),
            Some(hours(5)),
        )
        .expect("forecast");
        assert_close(f.rate_per_hour, 0.14, 1e-9);
        assert_close(f.projected, 1.165, 1e-9);
        assert_near_time(f.t_out.expect("t_out"), at_s(10, 10, 15, 19, 17, 0));
        assert!(f.valid && f.runs_out);
        assert_eq!(severity(78, &f, tn).severity, Severity::Warning);
        let gb = en_gb();
        assert_eq!(
            runout_text(f.t_out.unwrap(), false, &cx(tn, &gb, TimeStyle::Clock)),
            "Out 15:19"
        );
    }

    #[test]
    fn critical_by_level_and_runway_12_min_left() {
        let tn = at(10, 10, 14, 50);
        let f = stats::forecast(
            &readings(&[(at(10, 10, 14, 20), 0.79), (tn, 0.94)], r()),
            Some(r()),
            Some(hours(5)),
        )
        .expect("forecast");
        assert_close(f.rate_per_hour, 0.30, 1e-9);
        assert_close(f.projected, 1.44, 1e-9);
        assert_near_time(f.t_out.expect("t_out"), at(10, 10, 15, 2));
        let s = severity(94, &f, tn);
        assert_eq!(
            (s.severity, s.cause),
            (Severity::Critical, Some(CriticalCause::Runway))
        );
        let gb = en_gb();
        assert_eq!(
            runout_text(f.t_out.unwrap(), true, &cx(tn, &gb, TimeStyle::Clock)),
            "12 min left"
        );
    }

    #[test]
    fn critical_by_level_that_lasts() {
        let tn = at(10, 10, 16, 5);
        let f = stats::forecast(
            &readings(&[(at(10, 10, 15, 30), 0.92), (tn, 0.92)], r()),
            Some(r()),
            Some(hours(5)),
        )
        .expect("forecast");
        assert_eq!(f.rate_per_hour, 0.0);
        assert_close(f.projected, 0.92, 1e-9);
        assert_eq!(f.t_out, None);
        let s = severity(92, &f, tn);
        assert_eq!(
            (s.severity, s.cause),
            (Severity::Critical, Some(CriticalCause::Level))
        );
        let gb = en_gb();
        assert_eq!(time_token(r(), &cx(tn, &gb, TimeStyle::Clock)), "16:30");
    }

    #[test]
    fn ok_on_course_for_62_percent() {
        let reset = at(10, 10, 14, 56);
        let tn = now();
        let f = stats::forecast(
            &readings(&[(at(10, 10, 12, 12), 0.375), (tn, 0.42)], reset),
            Some(reset),
            Some(hours(5)),
        )
        .expect("forecast");
        assert_close(f.rate_per_hour, 0.09, 1e-9);
        assert_close(f.projected, 0.621, 1e-9);
        assert_near_time(f.t_out.expect("t_out"), at_s(10, 10, 19, 8, 40, 0));
        assert!(!f.runs_out);
        assert_eq!(severity(42, &f, tn).severity, Severity::Ok);
        let p = percent(share_pct(f.projected), &en_gb());
        assert_eq!(
            text(&English, Msg::CardForecastCourse, &[("p", &p)]),
            "on course for 62% at reset"
        );
    }

    #[test]
    fn no_forecast_in_a_fresh_window() {
        let reset = at(10, 10, 21, 30);
        let f = stats::forecast(
            &readings(
                &[(at(10, 10, 16, 31), 0.0), (at(10, 10, 16, 41), 0.06)],
                reset,
            ),
            Some(reset),
            Some(hours(5)),
        )
        .expect("forecast");
        assert!(!f.valid, "fresh-window guard, span 10 min");
    }

    #[test]
    fn warning_out_19_39_after_fifteen_minutes() {
        let reset = at(10, 10, 21, 30);
        let tn = at(10, 10, 16, 46);
        let f = stats::forecast(
            &readings(&[(at(10, 10, 16, 31), 0.0), (tn, 0.08)], reset),
            Some(reset),
            Some(hours(5)),
        )
        .expect("forecast");
        assert_close(f.rate_per_hour, 0.32, 1e-9);
        // p = .08 + .32 × 4 h 44 min = 1.594 67: the spec prints it to three places, 1.595.
        assert_close(f.projected, 1.595, 5e-4);
        assert_near_time(f.t_out.expect("t_out"), at_s(10, 10, 19, 38, 30, 0));
        assert_eq!(severity(8, &f, tn).severity, Severity::Warning);
        let gb = en_gb();
        // 19:38:30 rounds up
        assert_eq!(
            runout_text(f.t_out.unwrap(), false, &cx(tn, &gb, TimeStyle::Clock)),
            "Out 19:39"
        );
    }

    #[test]
    fn weekly_critical_six_hours_left() {
        // R Mon 09:00, at Sun 15:00: u .91, rate 1.5 %/h over S = 16.8 h → the anchor reading is at
        // Sun 15:00 − 16 h 48 min = Sat 22:12 with u = .91 − .015 × 16.8 = .658.
        let reset = at(10, 12, 9, 0);
        let tn = at(10, 11, 15, 0);
        let f = stats::forecast(
            &readings(&[(at(10, 10, 22, 12), 0.658), (tn, 0.91)], reset),
            Some(reset),
            Some(hours(7 * 24)),
        )
        .expect("forecast");
        assert_eq!(f.trailing, mins(16 * 60 + 48));
        assert_close(f.rate_per_hour, 0.015, 1e-9);
        assert_close(f.projected, 1.18, 1e-9);
        assert_near_time(f.t_out.expect("t_out"), at(10, 11, 21, 0));
        let s = severity(91, &f, tn);
        assert_eq!(s.severity, Severity::Critical);
        let gb = en_gb();
        assert_eq!(
            runout_text(f.t_out.unwrap(), true, &cx(tn, &gb, TimeStyle::Clock)),
            "6 h left"
        );
    }
}

// ---- Severity hysteresis (R§3.7) ------------------------------------------------------------------

mod hysteresis {
    use super::*;

    fn input(
        pct: u8,
        runs_out: bool,
        projected: f64,
        runway_min: Option<i64>,
    ) -> WindowSeverityInput {
        WindowSeverityInput {
            pct,
            exhausted: false,
            valid: true,
            runs_out,
            projected: Some(projected),
            runway_min,
        }
    }

    #[test]
    fn warning_enters_above_1_and_clears_at_0_97() {
        let a =
            stats::window_severity(&input(80, true, 1.02, Some(200)), SeverityMemory::default());
        assert_eq!(a.severity, Severity::Warning);
        let b = stats::window_severity(&input(81, false, 0.98, None), a.memory);
        assert_eq!(b.severity, Severity::Warning, "stays at p = 0.98");
        let c = stats::window_severity(&input(81, false, 0.97, None), b.memory);
        assert_eq!(c.severity, Severity::Ok, "clears at p = 0.97");
    }

    #[test]
    fn runway_critical_enters_at_30_stays_at_35_clears_at_36() {
        let never =
            stats::window_severity(&input(70, true, 1.5, Some(31)), SeverityMemory::default());
        assert_eq!(
            never.severity,
            Severity::Warning,
            "N 31 never enters: 31 min left with the triangle"
        );
        let a = stats::window_severity(&input(70, true, 1.5, Some(30)), SeverityMemory::default());
        assert_eq!(
            (a.severity, a.cause),
            (Severity::Critical, Some(CriticalCause::Runway))
        );
        let b = stats::window_severity(&input(70, true, 1.5, Some(35)), a.memory);
        assert_eq!(b.severity, Severity::Critical, "stays at N 35");
        let c = stats::window_severity(&input(70, true, 1.5, Some(36)), b.memory);
        assert_eq!(c.severity, Severity::Warning, "clears at N 36");
    }

    #[test]
    fn level_critical_enters_at_90_stays_at_88_clears_at_87() {
        let level = |pct| WindowSeverityInput {
            pct,
            exhausted: false,
            valid: false,
            runs_out: false,
            projected: None,
            runway_min: None,
        };
        let a = stats::window_severity(&level(90), SeverityMemory::default());
        assert_eq!(
            (a.severity, a.cause),
            (Severity::Critical, Some(CriticalCause::Level))
        );
        let b = stats::window_severity(&level(88), a.memory);
        assert_eq!(b.severity, Severity::Critical, "stays at 88");
        let c = stats::window_severity(&level(87), b.memory);
        assert_eq!(c.severity, Severity::Ok, "clears at 87");
    }
}

// ---- Balance (R§3.6, R§5.5) -----------------------------------------------------------------------

mod balance {
    use super::*;

    /// A top-up to 3,260 twenty days ago (from 50), then a 952-credit decrease over the last
    /// 7 days (136 a day). The extra sample one minute before the 7-day boundary keeps the older
    /// decrease out of the burn whichever way the boundary pair is counted.
    fn history(current: f64) -> Vec<Sample> {
        let start_of_span = current + 952.0;
        vec![
            Sample::balance(now() - hours(21 * 24), 50.0),
            Sample::balance(now() - hours(20 * 24), 3260.0),
            Sample::balance(now() - hours(7 * 24) - mins(1), start_of_span),
            Sample::balance(now() - hours(7 * 24), start_of_span),
            Sample::balance(now(), current),
        ]
    }

    #[test]
    fn credits_1240_of_3260_last_nine_days() {
        let b = stats::balance_stats(1240.0, None, &history(1240.0), now());
        assert_close(b.reference, 3260.0, 1e-9);
        assert_close(b.burn_per_day, 136.0, 1e-9);
        assert!(b.valid);
        assert_eq!(stats::display_pct(b.used_share, b.exhausted), 62);
        assert_close(b.days_left.expect("days_left"), 9.12, 0.005);
        assert_eq!(balance_duration(&b, &English).as_deref(), Some("9d"));
        let gb = en_gb();
        let date = card_date(b.t_out.expect("t_out"), &cx(now(), &gb, TimeStyle::Clock));
        assert_eq!(
            text(&English, Msg::CardBalanceUntil, &[("date", &date)]),
            "until ≈ Mon 19 Oct"
        );
    }

    #[test]
    fn credits_620_is_a_warning_out_thursday() {
        let b = stats::balance_stats(620.0, None, &history(620.0), now());
        assert_eq!(stats::display_pct(b.used_share, b.exhausted), 81);
        assert_close(b.days_left.expect("days_left"), 4.56, 0.005);
        // 620 / 136 d = 4 d 13 h 24 min 42 s after Sat 12:42 → Thu 15 Oct 02:06:42 (the spec
        // prints the minute it starts in, 02:06).
        assert_near_time(b.t_out.expect("t_out"), at_s(10, 15, 2, 6, 42, 0));
        let s = stats::balance_severity(
            &BalanceSeverityInput {
                exhausted: false,
                days_left: b.days_left,
            },
            SeverityMemory::default(),
            None,
        );
        assert_eq!(s.severity, Severity::Warning);
        let gb = en_gb();
        assert_eq!(
            runout_text(b.t_out.unwrap(), false, &cx(now(), &gb, TimeStyle::Clock)),
            "Out Thu"
        );
    }
}

// ---- Freshness (R§3.8) ----------------------------------------------------------------------------

mod freshness {
    use super::*;

    #[test]
    fn stale_after() {
        assert_eq!(stats::stale_after(Some(mins(5))), mins(15));
        assert_eq!(stats::stale_after(Some(mins(10))), mins(20));
        assert_eq!(stats::stale_after(None), mins(60));
    }

    #[test]
    fn http_401_is_stale_sign_in_at_once() {
        let status = ServiceStatus {
            last_success: Some(now() - mins(1)),
            last_failure: Some((now(), FailureKind::Auth)),
            poll_interval: Some(mins(5)),
            ..ServiceStatus::default()
        };
        assert_eq!(
            stats::freshness(now(), &status),
            Freshness::Stale(StaleReason::SignIn)
        );
    }

    #[test]
    fn sixteen_minutes_without_a_reading_at_a_five_minute_interval() {
        let fresh = ServiceStatus {
            last_success: Some(now() - mins(15)),
            poll_interval: Some(mins(5)),
            ..ServiceStatus::default()
        };
        assert_eq!(stats::freshness(now(), &fresh), Freshness::Fresh);
        let stale = ServiceStatus {
            last_success: Some(now() - mins(16)),
            ..fresh
        };
        assert_eq!(
            stats::freshness(now(), &stale),
            Freshness::Stale(StaleReason::NoData)
        );
    }
}
