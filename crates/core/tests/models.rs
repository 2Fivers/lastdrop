//! End-to-end checks of `lastdrop-core`: snapshots → `stats::evaluate` → the readout, hover card,
//! tray and flyout models, against the examples of readout-spec §5–§11 and flyout-spec §3–§10,
//! and the edge cases those sections name (just reset, starts on use, unknown window length,
//! used > 1, 0 % and 100 %, no reset time, balances, stale, several accounts, 12/24-hour locales,
//! hysteresis entering and leaving).
//!
//! All data is invented. "Local" is a fixed UTC+02:00 zone, `now` is Sat 2026-10-10 12:42 unless
//! a test says otherwise (as in R§13).

use std::collections::BTreeMap;

use jiff::tz::{self, TimeZone};
use jiff::{SignedDuration, Timestamp, civil};
use lastdrop_core::color::ColorRole;
use lastdrop_core::flyout::{
    CardBody, MessageKind, RightKind, SparkKind, Tone, build_flyout, spark_values,
};
use lastdrop_core::glyph::SeverityGlyph;
use lastdrop_core::grammar::{English, LocaleFormat, TextContext, TimeStyle};
use lastdrop_core::model::{
    AccountKey, AccountRef, Balance, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId,
    Snapshot, SourceKind, Unit, WindowLen,
};
use lastdrop_core::readout::bar::{BarMode, bar_geometry};
use lastdrop_core::readout::hovercard::CardMeter;
use lastdrop_core::readout::{
    BuildContext, ContentForm, HoverAnchor, ReadoutModel, Run, TrayIcon, build_hover_card,
    build_readout, build_tray,
};
use lastdrop_core::settings::{Pause, ProviderSettings, ReadoutContent, Settings};
use lastdrop_core::stats::{
    self, AssessedMeter, DisplayClass, EvalInput, EvalMemory, Evaluation, FailureKind, LiveChange,
    ResetView, RowState, Sample, ServiceInput, ServiceStatus, Severity, StaleReason,
};

// ---- fixtures -----------------------------------------------------------------------------------

fn zone() -> TimeZone {
    TimeZone::fixed(tz::offset(2))
}

fn at(month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
    civil::date(2026, month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(zone())
        .expect("valid local time")
        .timestamp()
}

fn now() -> Timestamp {
    at(10, 10, 12, 42)
}

fn mins(m: i64) -> SignedDuration {
    SignedDuration::from_mins(m)
}

fn hours(h: i64) -> SignedDuration {
    SignedDuration::from_hours(h)
}

fn names(short_label: &str, short_name: &str, name: &str) -> MeterNames {
    MeterNames::new(short_label, short_name, name)
}

fn window_meter(
    id: &str,
    names: MeterNames,
    order: u16,
    used: f64,
    resets_at: Option<Timestamp>,
    window: WindowLen,
    starts_on_use: bool,
) -> Meter {
    Meter {
        id: MeterId::new(id),
        names,
        order,
        used,
        limited: false,
        resets_at,
        window,
        starts_on_use,
    }
}

fn five_hour(used: f64, resets_at: Option<Timestamp>) -> Meter {
    window_meter(
        MeterId::FIVE_HOUR,
        names("5h", "5-hour", "5-hour session"),
        0,
        used,
        resets_at,
        WindowLen::Fixed(hours(5)),
        true,
    )
}

fn weekly(used: f64, resets_at: Option<Timestamp>) -> Meter {
    window_meter(
        MeterId::WEEKLY,
        names("Wk", "weekly", "Weekly · all models"),
        1,
        used,
        resets_at,
        WindowLen::Fixed(hours(7 * 24)),
        false,
    )
}

fn codex_weekly(used: f64, resets_at: Option<Timestamp>) -> Meter {
    window_meter(
        MeterId::WEEKLY,
        names("Wk", "weekly", "Weekly"),
        1,
        used,
        resets_at,
        WindowLen::Fixed(hours(7 * 24)),
        false,
    )
}

fn credits(amount: f64) -> Balance {
    Balance {
        id: MeterId::new(MeterId::CREDITS),
        names: names("Cr", "credits", "Credits"),
        order: 0,
        amount,
        unit: Unit::Credits,
        last_top_up: None,
    }
}

fn account(key: &str) -> AccountRef {
    AccountRef {
        key: AccountKey(key.to_owned()),
        ..AccountRef::primary()
    }
}

fn snapshot(provider: ProviderId, meters: Vec<Meter>, t: Timestamp) -> Snapshot {
    Snapshot {
        provider,
        account: AccountRef::primary(),
        source: match provider {
            ProviderId::Claude => SourceKind::ClaudeOAuthUsage,
            ProviderId::Codex => SourceKind::CodexAppServer,
            ProviderId::Meshy => SourceKind::MeshyApi,
        },
        measured_at: t,
        fetched_at: t,
        meters,
        balances: Vec::new(),
        health: Health::Ok,
    }
}

fn meshy(amount: f64, t: Timestamp) -> Snapshot {
    Snapshot {
        balances: vec![credits(amount)],
        ..snapshot(ProviderId::Meshy, Vec::new(), t)
    }
}

fn key(provider: ProviderId, meter: &str) -> MeterKey {
    MeterKey {
        provider,
        account: AccountKey::primary(),
        meter: MeterId::new(meter),
    }
}

fn fresh(t: Timestamp) -> ServiceStatus {
    ServiceStatus {
        last_success: Some(t),
        poll_interval: Some(mins(5)),
        ..ServiceStatus::default()
    }
}

fn settings_with(providers: &[ProviderId]) -> Settings {
    let mut settings = Settings::default();
    settings.providers = providers
        .iter()
        .map(|p| lastdrop_core::settings::ProviderEntry::Known(ProviderSettings::new(*p, true)))
        .collect();
    settings
}

/// One service of a scenario.
struct Svc {
    provider: ProviderId,
    account: AccountRef,
    snapshot: Option<Snapshot>,
    status: ServiceStatus,
    fetching: bool,
}

impl Svc {
    fn new(snapshot: Snapshot) -> Svc {
        let t = snapshot.measured_at;
        Svc {
            provider: snapshot.provider,
            account: snapshot.account.clone(),
            snapshot: Some(snapshot),
            status: fresh(t),
            fetching: false,
        }
    }

    fn loading(provider: ProviderId) -> Svc {
        Svc {
            provider,
            account: AccountRef::primary(),
            snapshot: None,
            status: ServiceStatus::default(),
            fetching: true,
        }
    }

    fn status(mut self, status: ServiceStatus) -> Svc {
        self.status = status;
        self
    }
}

type History = BTreeMap<MeterKey, Vec<Sample>>;

fn evaluate(
    services: &[Svc],
    settings: &Settings,
    history: &History,
    now: Timestamp,
    memory: &mut EvalMemory,
) -> Evaluation {
    let inputs: Vec<ServiceInput<'_>> = services
        .iter()
        .map(|s| ServiceInput {
            provider: s.provider,
            account: &s.account,
            snapshot: s.snapshot.as_ref(),
            status: &s.status,
            fetching: s.fetching,
        })
        .collect();
    stats::evaluate(
        &EvalInput {
            now,
            settings,
            services: &inputs,
            history,
        },
        memory,
    )
}

fn eval_once(
    services: &[Svc],
    settings: &Settings,
    history: &History,
    now: Timestamp,
) -> Evaluation {
    evaluate(services, settings, history, now, &mut EvalMemory::default())
}

fn text_cx(now: Timestamp, locale: &LocaleFormat) -> TextContext<'_> {
    TextContext {
        now,
        time_style: TimeStyle::Clock,
        locale,
        strings: &English,
    }
}

fn build_cx<'a>(
    now: Timestamp,
    locale: &'a LocaleFormat,
    settings: &'a Settings,
) -> BuildContext<'a> {
    BuildContext {
        text: text_cx(now, locale),
        settings,
    }
}

fn gb() -> LocaleFormat {
    LocaleFormat::en_gb(zone())
}

fn us() -> LocaleFormat {
    LocaleFormat::en_us(zone())
}

/// History of a 5-hour window resetting at 16:30 with the two readings of a forecast vector.
fn five_hour_history(readings: &[(Timestamp, f64)], reset: Timestamp) -> History {
    let mut h = History::new();
    h.insert(
        key(ProviderId::Claude, MeterId::FIVE_HOUR),
        readings
            .iter()
            .map(|&(t, u)| Sample::window(t, u, Some(reset)))
            .collect(),
    );
    h
}

/// The cell texts of a readout row, glyphs as `[glyph]`.
fn cell_texts(model: &ReadoutModel, row: usize) -> Vec<String> {
    let cell = &model.rows[row].right;
    let mut out: Vec<String> = cell.value.iter().map(|v| v.text.clone()).collect();
    for run in cell.say.iter().chain(&cell.time) {
        out.push(match run {
            Run::Text(t) => t.text.clone(),
            Run::Glyph(_) => "[glyph]".to_owned(),
        });
    }
    out
}

fn compact_texts(model: &ReadoutModel, row: usize) -> Vec<String> {
    model.rows[row]
        .compact
        .runs
        .iter()
        .map(|r| match r {
            Run::Text(t) => t.text.clone(),
            Run::Glyph(_) => "[glyph]".to_owned(),
        })
        .collect()
}

// ---- R§11 sentences, 12-hour locale ------------------------------------------------------------

mod sentences {
    use super::*;
    use lastdrop_core::grammar::a11y::{row_sentence, service_sentence};

    fn sentence_of(services: &[Svc], history: &History, now: Timestamp, k: &MeterKey) -> String {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Meshy]);
        let eval = eval_once(services, &settings, history, now);
        let (service, a) = eval.assessment(k).expect("assessed");
        let locale = us();
        row_sentence(a, service, &text_cx(now, &locale))
    }

    #[test]
    fn on_track() {
        let snap = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.42, Some(at(10, 10, 14, 56)))],
            now(),
        );
        assert_eq!(
            sentence_of(
                &[Svc::new(snap)],
                &History::new(),
                now(),
                &key(ProviderId::Claude, "five_hour")
            ),
            "Claude 5-hour session: on track, 42 percent used, resets at 2:56 PM."
        );
    }

    #[test]
    fn warning_runway_level_and_exhausted() {
        let r = at(10, 10, 16, 30);
        let k = key(ProviderId::Claude, "five_hour");

        let tn = at(10, 10, 13, 45);
        let h = five_hour_history(&[(at(10, 10, 13, 15), 0.71), (tn, 0.78)], r);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(0.78, Some(r))], tn);
        assert_eq!(
            sentence_of(&[Svc::new(snap)], &h, tn, &k),
            "Claude 5-hour session: warning, runs out at about 3:19 PM, before the reset at 4:30 PM; \
             78 percent used."
        );

        let tn = at(10, 10, 14, 50);
        let h = five_hour_history(&[(at(10, 10, 14, 20), 0.79), (tn, 0.94)], r);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(0.94, Some(r))], tn);
        assert_eq!(
            sentence_of(&[Svc::new(snap)], &h, tn, &k),
            "Claude 5-hour session: critical, runs out in about 12 minutes; 94 percent used; \
             resets at 4:30 PM."
        );

        let tn = at(10, 10, 16, 5);
        let h = five_hour_history(&[(at(10, 10, 15, 30), 0.92), (tn, 0.92)], r);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(0.92, Some(r))], tn);
        assert_eq!(
            sentence_of(&[Svc::new(snap)], &h, tn, &k),
            "Claude 5-hour session: critical, 92 percent used, lasts until the reset at 4:30 PM."
        );

        let tn = at(10, 10, 14, 50);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(1.0, Some(r))], tn);
        assert_eq!(
            sentence_of(&[Svc::new(snap)], &History::new(), tn, &k),
            "Claude 5-hour session: empty, back at 4:30 PM."
        );
    }

    #[test]
    fn weekly_critical_resets_on_monday() {
        let reset = at(10, 12, 9, 0);
        let tn = at(10, 11, 15, 0);
        let mut h = History::new();
        h.insert(
            key(ProviderId::Claude, "weekly"),
            vec![
                Sample::window(at(10, 10, 22, 12), 0.658, Some(reset)),
                Sample::window(tn, 0.91, Some(reset)),
            ],
        );
        let snap = snapshot(ProviderId::Claude, vec![weekly(0.91, Some(reset))], tn);
        assert_eq!(
            sentence_of(
                &[Svc::new(snap)],
                &h,
                tn,
                &key(ProviderId::Claude, "weekly")
            ),
            "Claude weekly, all models: critical, runs out in about 6 hours; 91 percent used; \
             resets Monday 9:00 AM."
        );
    }

    #[test]
    fn not_started() {
        let snap = snapshot(ProviderId::Claude, vec![five_hour(0.0, None)], now());
        assert_eq!(
            sentence_of(
                &[Svc::new(snap)],
                &History::new(),
                now(),
                &key(ProviderId::Claude, "five_hour")
            ),
            "Claude 5-hour session: not started, 0 percent used, starts with your next message."
        );
    }

    #[test]
    fn stale_services() {
        let read = now() - mins(47);
        let snap = snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.42, Some(at(10, 10, 14, 56))),
                weekly(0.61, Some(at(10, 12, 9, 0))),
            ],
            read,
        );
        let offline = ServiceStatus {
            last_failure: Some((now() - mins(2), FailureKind::Offline)),
            ..fresh(read)
        };
        let settings = settings_with(&[ProviderId::Claude]);
        let eval = eval_once(
            &[Svc::new(snap.clone()).status(offline)],
            &settings,
            &History::new(),
            now(),
        );
        let service = &eval.services[0];
        let rows: Vec<_> = service.meters.iter().collect();
        let locale = us();
        assert_eq!(
            service_sentence(service, &rows, &text_cx(now(), &locale)),
            "Claude: offline, last reading 47 minutes ago. Then: 5-hour session 42 percent used, \
             weekly, all models 61 percent used."
        );
        let sign_in = ServiceStatus {
            last_failure: Some((now(), FailureKind::Auth)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[Svc::new(snap).status(sign_in)],
            &settings,
            &History::new(),
            now(),
        );
        let service = &eval.services[0];
        let rows: Vec<_> = service.meters.iter().collect();
        assert_eq!(
            service_sentence(service, &rows, &text_cx(now(), &locale)),
            "Claude: sign-in expired, last reading 47 minutes ago. Select to open Lastdrop and see \
             how to sign in."
        );
    }

    #[test]
    fn balance() {
        let mut h = History::new();
        h.insert(key(ProviderId::Meshy, "credits"), meshy_history(1240.0));
        assert_eq!(
            sentence_of(
                &[Svc::new(meshy(1240.0, now()))],
                &h,
                now(),
                &key(ProviderId::Meshy, "credits")
            ),
            "Meshy: 1,240 credits left, about 9 days at the current rate."
        );
    }
}

/// The balance history of R§13's balance vector: a top-up to 3,260 twenty days ago, then 952
/// credits spent over the last 7 days (136 a day).
fn meshy_history(current: f64) -> Vec<Sample> {
    let start_of_span = current + 952.0;
    vec![
        Sample::balance(now() - hours(21 * 24), 50.0),
        Sample::balance(now() - hours(20 * 24), 3260.0),
        Sample::balance(now() - hours(7 * 24) - mins(1), start_of_span),
        Sample::balance(now() - hours(7 * 24), start_of_span),
        Sample::balance(now(), current),
    ]
}

// ---- readout model ---------------------------------------------------------------------------

mod readout {
    use super::*;

    fn claude_and_codex() -> Vec<Svc> {
        vec![
            Svc::new(snapshot(
                ProviderId::Claude,
                vec![
                    five_hour(0.42, Some(at(10, 10, 14, 56))),
                    weekly(0.61, Some(at(10, 12, 9, 0))),
                ],
                now() - mins(1),
            )),
            Svc::new(snapshot(
                ProviderId::Codex,
                vec![codex_weekly(0.10, Some(at(10, 16, 14, 26)))],
                now() - mins(1),
            )),
        ]
    }

    #[test]
    fn auto_rows_cells_glyph_column_and_widths() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Codex]);
        let eval = eval_once(&claude_and_codex(), &settings, &History::new(), now());
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.content, ContentForm::TwoRows);
        assert!(model.glyph_column, "two services can appear");
        assert_eq!(model.rows.len(), 2);
        // The two most constrained (Claude weekly .61 and 5-hour .42), in service/meter order.
        assert_eq!(model.rows[0].key, key(ProviderId::Claude, "five_hour"));
        assert_eq!(model.rows[1].key, key(ProviderId::Claude, "weekly"));
        assert_eq!(model.rows[0].label.text, "5h");
        assert_eq!(model.rows[0].label.role, ColorRole::TextSecondary);
        assert!(model.rows[0].glyph.is_some());
        assert_eq!(cell_texts(&model, 0), ["42%", "[glyph]", "14:56"]);
        assert_eq!(cell_texts(&model, 1), ["61%", "[glyph]", "Mon"]);
        assert_eq!(compact_texts(&model, 1), ["61%"]);
        // The compact form shows the winner among the rows: the weekly (q .61).
        assert_eq!(model.compact_row, 1);
        assert_eq!(model.rows[1].uia_value, Some(61));
        assert!(
            model
                .uia
                .root_name
                .starts_with("Lastdrop. Claude 5-hour session: on track")
        );
        assert_eq!(
            model.uia.help_text,
            "Select for details, right-click for options."
        );
        assert!(!model.uia.announce);
        // Widths come from what can appear, not from values.
        let w = &model.widths;
        assert!(w.glyph_column);
        assert_eq!(w.labels, ["5h", "Wk"]);
        assert!(w.time_tokens.contains(&"00:00".to_owned()));
        assert!(w.time_tokens.contains(&"Today".to_owned()));
        assert!(w.time_tokens.contains(&"88d".to_owned()));
        assert!(w.say_semibold.contains(&"59 min left".to_owned()));
        assert!(w.say_semibold.contains(&"Back Today".to_owned()));
        assert!(w.statuses.contains(&"59m ago".to_owned()));
        assert!(w.regular_at_zero.contains(&"Reading…".to_owned()));
        assert_eq!(w.balance, None, "no balance service enabled");
        assert_eq!(w.compact_regular, ["99%"]);

        // The same evaluation with other values gives the same widths.
        let mut other = claude_and_codex();
        other[0] = Svc::new(snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.97, Some(at(10, 10, 13, 0))),
                weekly(0.2, Some(at(10, 12, 9, 0))),
            ],
            now() - mins(1),
        ));
        let eval2 = eval_once(&other, &settings, &History::new(), now());
        let model2 = build_readout(&eval2, &build_cx(now(), &locale, &settings));
        assert_eq!(model2.widths, model.widths);
    }

    #[test]
    fn twelve_hour_locale_drops_am_pm_in_the_readout() {
        let settings = settings_with(&[ProviderId::Claude]);
        let snap = snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.42, Some(at(10, 10, 14, 56))),
                weekly(0.61, Some(at(10, 12, 9, 0))),
            ],
            now(),
        );
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let locale = us();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert!(!model.glyph_column, "one service: labels only");
        assert_eq!(cell_texts(&model, 0), ["42%", "[glyph]", "2:56"]);
        assert!(model.widths.time_tokens.contains(&"12:00".to_owned()));
        assert!(model.rows[0].uia_name.ends_with("resets at 2:56 PM."));
    }

    #[test]
    fn pinned_rows_keep_the_users_order_and_off_hides() {
        let mut settings = settings_with(&[ProviderId::Claude, ProviderId::Codex]);
        settings.readout.content = ReadoutContent::Pinned(vec![
            key(ProviderId::Codex, "weekly"),
            key(ProviderId::Claude, "five_hour"),
        ]);
        let eval = eval_once(&claude_and_codex(), &settings, &History::new(), now());
        assert_eq!(
            eval.readout_rows,
            [
                key(ProviderId::Codex, "weekly"),
                key(ProviderId::Claude, "five_hour")
            ]
        );
        assert!(eval.multi_service);
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.widths.labels, ["5h", "Wk"]);

        settings.readout.content = ReadoutContent::Pinned(vec![key(ProviderId::Claude, "weekly")]);
        let eval = eval_once(&claude_and_codex(), &settings, &History::new(), now());
        assert!(!eval.multi_service, "one pin: one service");
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.content, ContentForm::SingleRow);

        settings.readout.enabled = false;
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.content, ContentForm::Hidden);
    }

    #[test]
    fn loading_rows_then_nothing_after_a_failed_first_fetch() {
        let settings = settings_with(&[ProviderId::Claude]);
        let eval = eval_once(
            &[Svc::loading(ProviderId::Claude)],
            &settings,
            &History::new(),
            now(),
        );
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.content, ContentForm::TwoRows);
        assert_eq!(cell_texts(&model, 0), ["Reading…"]);
        assert_eq!(cell_texts(&model, 1), ["–"]);
        assert_eq!(compact_texts(&model, 0), ["–"]);
        assert_eq!(model.rows[0].uia_value, None);
        let g = bar_geometry(&model.rows[0].bar, 40.0, 16.0, 1.0);
        assert!(g.fill.is_empty() && g.tick.is_none());
        assert_eq!(eval.winner, Some(key(ProviderId::Claude, "five_hour")));

        let failed = Svc {
            fetching: false,
            status: ServiceStatus {
                last_failure: Some((now(), FailureKind::NoData)),
                failure_detail: Some("HTTP 503".to_owned()),
                ..ServiceStatus::default()
            },
            ..Svc::loading(ProviderId::Claude)
        };
        let eval = eval_once(&[failed], &settings, &History::new(), now());
        assert!(eval.services[0].first_fetch_failed);
        assert_eq!(eval.winner, None);
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.content, ContentForm::Hidden);
    }

    #[test]
    fn stale_rows_take_reason_then_age() {
        let settings = settings_with(&[ProviderId::Claude]);
        let read = now() - mins(47);
        let snap = snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.42, Some(at(10, 10, 14, 56))),
                weekly(0.61, Some(at(10, 12, 9, 0))),
            ],
            read,
        );
        let offline = ServiceStatus {
            last_failure: Some((now(), FailureKind::Offline)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[Svc::new(snap).status(offline)],
            &settings,
            &History::new(),
            now(),
        );
        assert_eq!(
            eval.services[0].meters[0].state,
            RowState::Stale(StaleReason::Offline)
        );
        assert_eq!(eval.services[0].meters[0].class, DisplayClass::Stale);
        assert_eq!(eval.services[0].meters[0].severity, None);
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(cell_texts(&model, 0), ["42%", "Offline"]);
        assert_eq!(cell_texts(&model, 1), ["61%", "47m ago"]);
        assert_eq!(
            model.rows[0].right.value.as_ref().map(|v| v.role),
            Some(ColorRole::Stale)
        );
        // The bar is dashed and frozen at the reading: e(t_r).
        let bar = &model.rows[0].bar;
        assert!(bar.dashed);
        assert_eq!(bar.fill, ColorRole::Stale);
        let e = bar.elapsed.expect("tick");
        assert!((e - stats::elapsed_share(read, at(10, 10, 14, 56), hours(5))).abs() < 1e-12);
        // One sentence for the stale service.
        assert!(
            model
                .uia
                .root_name
                .starts_with("Lastdrop. Claude: offline, last reading 47 minutes ago.")
        );
    }

    #[test]
    fn one_row_stale_service_shows_sign_in_and_a_frozen_used_up_row_shows_back() {
        let mut settings = settings_with(&[ProviderId::Claude]);
        settings.readout.content =
            ReadoutContent::Pinned(vec![key(ProviderId::Claude, "five_hour")]);
        let read = now() - mins(30);
        let snap = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.42, Some(at(10, 10, 14, 56)))],
            read,
        );
        let sign_in = ServiceStatus {
            last_failure: Some((now(), FailureKind::Auth)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[Svc::new(snap).status(sign_in)],
            &settings,
            &History::new(),
            now(),
        );
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(cell_texts(&model, 0), ["42%", "Sign in"]);
        match &model.rows[0].right.time[0] {
            Run::Text(t) => assert_eq!(t.role, ColorRole::AccentText),
            Run::Glyph(_) => panic!("expected text"),
        }

        // Two rows, the 5-hour frozen used up: `Back 14:56` from x = 0, the weekly takes the
        // one-row status (Offline → AGE).
        settings.readout.content = ReadoutContent::Auto;
        let mut used_up = five_hour(1.0, Some(at(10, 10, 14, 56)));
        used_up.limited = true;
        let snap = snapshot(
            ProviderId::Claude,
            vec![used_up, weekly(0.61, Some(at(10, 12, 9, 0)))],
            read,
        );
        let offline = ServiceStatus {
            last_failure: Some((now(), FailureKind::Offline)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[Svc::new(snap).status(offline)],
            &settings,
            &History::new(),
            now(),
        );
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(cell_texts(&model, 0), ["Back 14:56"]);
        match &model.rows[0].right.say[0] {
            Run::Text(t) => assert_eq!(t.role, ColorRole::Stale),
            Run::Glyph(_) => panic!("no glyph on a stale row"),
        }
        assert_eq!(cell_texts(&model, 1), ["61%", "30m ago"]);
        assert_eq!(model.rows[0].bar.mode, BarMode::Exhausted);
    }

    #[test]
    fn several_accounts_get_tags_and_aliases() {
        let settings = settings_with(&[ProviderId::Claude]);
        let mut work = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.42, Some(at(10, 10, 14, 56)))],
            now(),
        );
        work.account = account("work");
        let mut personal = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.30, Some(at(10, 10, 15, 0)))],
            now(),
        );
        personal.account = account("personal");
        let mut svc_work = Svc::new(work);
        svc_work.account.alias = Some("Work".to_owned());
        let mut svc_personal = Svc::new(personal);
        svc_personal.account.alias = Some("Personal".to_owned());
        svc_personal.account.tag = Some("Pe".to_owned());
        let eval = eval_once(&[svc_work, svc_personal], &settings, &History::new(), now());
        assert!(!eval.multi_service, "one service, two accounts");
        assert_eq!(eval.services[0].display_name(), "Claude · Work");
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        let labels: Vec<&str> = model.rows.iter().map(|r| r.label.text.as_str()).collect();
        assert_eq!(labels, ["W 5h", "Pe 5h"]);
        assert!(model.widths.labels.contains(&"W 5h".to_owned()));
        assert!(
            model.rows[0]
                .uia_name
                .starts_with("Claude · Work 5-hour session:")
        );
    }

    #[test]
    fn balance_fills_an_empty_place_and_widens_the_cell() {
        let settings = settings_with(&[ProviderId::Meshy]);
        let mut h = History::new();
        h.insert(key(ProviderId::Meshy, "credits"), meshy_history(1240.0));
        let eval = eval_once(&[Svc::new(meshy(1240.0, now()))], &settings, &h, now());
        assert_eq!(eval.readout_rows, [key(ProviderId::Meshy, "credits")]);
        assert_eq!(eval.winner, Some(key(ProviderId::Meshy, "credits")));
        let locale = gb();
        let model = build_readout(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(model.content, ContentForm::SingleRow);
        assert_eq!(cell_texts(&model, 0), ["62%", "≈", "9d"]);
        let geom = bar_geometry(&model.rows[0].bar, 40.0, 16.0, 1.0);
        assert_eq!(geom.tick, None, "a balance has no tick");
        let balance = model.widths.balance.as_ref().expect("a balance can appear");
        assert_eq!(balance.approx, "≈");
        assert_eq!(balance.durations, ["47h", "13d", "8w", "9w+"]);
    }

    #[test]
    fn warning_balance_counts_down_like_a_window() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Meshy]);
        let mut h = History::new();
        h.insert(key(ProviderId::Meshy, "credits"), meshy_history(620.0));
        let services = [
            Svc::new(snapshot(
                ProviderId::Claude,
                vec![
                    five_hour(0.42, Some(at(10, 10, 14, 56))),
                    weekly(0.61, Some(at(10, 12, 9, 0))),
                ],
                now(),
            )),
            Svc::new(meshy(620.0, now())),
        ];
        let eval = eval_once(&services, &settings, &h, now());
        let (_, credits) = eval
            .assessment(&key(ProviderId::Meshy, "credits"))
            .expect("assessed");
        assert_eq!(credits.state, RowState::Warning);
        assert_eq!(eval.winner, Some(key(ProviderId::Meshy, "credits")));
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let model = build_readout(&eval, &cx);
        let row = model
            .rows
            .iter()
            .position(|r| r.key == key(ProviderId::Meshy, "credits"))
            .expect("shown");
        assert_eq!(cell_texts(&model, row), ["[glyph]", "Out Thu"]);
        assert_eq!(compact_texts(&model, row), ["[glyph]", "81%"]);
        let tray = build_tray(&eval, &cx);
        assert_eq!(
            tray.icon,
            TrayIcon::CautionTile {
                digits: "81".to_owned()
            }
        );
    }
}

// ---- states: just reset, starts on use, unknown window, used > 1 -------------------------------

mod states {
    use super::*;

    #[test]
    fn after_the_reset_without_a_new_reading() {
        let settings = settings_with(&[ProviderId::Claude]);
        let now = at(10, 12, 9, 1);
        let snap = snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.80, Some(at(10, 12, 8, 30))),
                weekly(0.61, Some(at(10, 12, 9, 0))),
            ],
            at(10, 12, 8, 58),
        );
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now);
        let (_, five) = eval
            .assessment(&key(ProviderId::Claude, "five_hour"))
            .expect("5h");
        let (_, week) = eval
            .assessment(&key(ProviderId::Claude, "weekly"))
            .expect("weekly");
        assert_eq!(five.state, RowState::Idle);
        assert_eq!(week.state, RowState::JustReset);
        assert_eq!(week.severity, Some(Severity::Ok));
        match &week.meter {
            AssessedMeter::Window(w) => {
                assert_eq!(
                    w.reset,
                    ResetView::JustReset {
                        previous: at(10, 12, 9, 0)
                    }
                );
                assert_eq!(w.resets_at, Some(at(10, 19, 9, 0)));
                assert!(w.forecast.is_none(), "guards after a reset");
            }
            other => panic!("expected a window, got {other:?}"),
        }
        match &five.meter {
            AssessedMeter::Window(w) => {
                assert_eq!(
                    w.reset,
                    ResetView::IdleAfterReset {
                        previous: at(10, 12, 8, 30)
                    }
                );
                assert_eq!((w.used, w.resets_at, w.elapsed), (0.0, None, None));
            }
            other => panic!("expected a window, got {other:?}"),
        }
        let locale = gb();
        let cx = build_cx(now, &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        let lines: Vec<(String, String)> = card.services[0]
            .meters
            .iter()
            .map(|m| match m {
                CardMeter::Window(w) => (w.elapsed_line.clone(), w.reset_line.clone()),
                CardMeter::Balance(_) => panic!("window expected"),
            })
            .collect();
        assert_eq!(
            lines,
            [
                (
                    "window not started".to_owned(),
                    "Reset at 08:30 · waiting for the first reading".to_owned()
                ),
                (
                    "0% of time gone".to_owned(),
                    "Reset at 09:00 · waiting for the first reading".to_owned()
                ),
            ]
        );
        let flyout = build_flyout(&eval, &History::new(), false, &cx);
        let CardBody::Meters(rows) = &flyout.cards[0].body else {
            panic!("meters expected");
        };
        assert_eq!(rows[1].right.text, "Resets Mon 19 Oct");
        assert_eq!(rows[1].right.kind, RightKind::Reset);
        assert_eq!(
            rows[1].line.as_deref(),
            Some("Reset at 09:00 · waiting for the first reading")
        );
        assert_eq!(rows[0].right.text, "Starts with your next use");
        assert_eq!(rows[0].line, None);
    }

    #[test]
    fn unknown_window_length_shows_the_reset_without_a_tick() {
        let settings = settings_with(&[ProviderId::Claude]);
        let r = at(10, 10, 18, 0);
        let meter = window_meter(
            "daily",
            names("Dy", "daily", "Daily"),
            2,
            0.30,
            Some(r),
            WindowLen::Unknown,
            false,
        );
        let snap = snapshot(ProviderId::Claude, vec![meter], now());
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let (_, a) = eval
            .assessment(&key(ProviderId::Claude, "daily"))
            .expect("assessed");
        assert_eq!(a.state, RowState::Ok);
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let model = build_readout(&eval, &cx);
        assert_eq!(cell_texts(&model, 0), ["30%", "[glyph]", "18:00"]);
        assert_eq!(bar_geometry(&model.rows[0].bar, 40.0, 16.0, 1.0).tick, None);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        let CardMeter::Window(w) = &card.services[0].meters[0] else {
            panic!("window expected");
        };
        assert_eq!(w.elapsed_line, "window length unknown");
        assert_eq!(w.reset_line, "Resets 18:00");
        assert_eq!(w.reset_in.as_deref(), Some("in 5 h 18 min"));
        assert!(
            model.widths.labels.contains(&"Dy".to_owned()),
            "reported meters widen LW"
        );
    }

    #[test]
    fn used_above_one_is_exhausted_at_100() {
        let settings = settings_with(&[ProviderId::Claude]);
        let r = at(10, 10, 16, 30);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(1.3, Some(r))], now());
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let (_, a) = eval
            .assessment(&key(ProviderId::Claude, "five_hour"))
            .expect("assessed");
        assert_eq!(a.state, RowState::Exhausted);
        assert_eq!(a.class, DisplayClass::Exhausted);
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let model = build_readout(&eval, &cx);
        assert_eq!(cell_texts(&model, 0), ["[glyph]", "Back 16:30"]);
        assert_eq!(compact_texts(&model, 0), ["[glyph]", "Empty"]);
        assert_eq!(model.rows[0].uia_value, Some(100));
        let g = bar_geometry(&model.rows[0].bar, 40.0, 16.0, 1.0);
        assert_eq!(g.fill_end, 40);
        assert_eq!(build_tray(&eval, &cx).icon, TrayIcon::ExhaustedTile);
        let flyout = build_flyout(&eval, &History::new(), false, &cx);
        let CardBody::Meters(rows) = &flyout.cards[0].body else {
            panic!("meters expected");
        };
        assert_eq!(rows[0].value, "100%", "100% is allowed in the flyout");
        assert_eq!(rows[0].tone, Tone::Critical);
        assert_eq!(flyout.verdict.text, "Claude 5-hour is used up until 16:30");
        assert_eq!(
            rows[0].line.as_deref(),
            Some("Used up. Back in 3 h 48 min.")
        );
    }

    #[test]
    fn no_reset_time_with_something_used() {
        let settings = settings_with(&[ProviderId::Claude]);
        let snap = snapshot(ProviderId::Claude, vec![weekly(0.40, None)], now());
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        let CardMeter::Window(w) = &card.services[0].meters[0] else {
            panic!("window expected");
        };
        assert_eq!(w.reset_line, "No reset time given");
        assert_eq!(w.reset_in, None);
        assert_eq!(w.forecast, None);
        let flyout = build_flyout(&eval, &History::new(), false, &cx);
        let CardBody::Meters(rows) = &flyout.cards[0].body else {
            panic!("meters expected");
        };
        assert_eq!(rows[0].right.text, "No reset time given");
        assert_eq!(rows[0].right.kind, RightKind::Plain);
    }
}

// ---- hysteresis and announcements through evaluate ---------------------------------------------

mod memory {
    use super::*;

    fn weekly_at(used: f64, reset: Timestamp, t: Timestamp) -> Vec<Svc> {
        vec![Svc::new(snapshot(
            ProviderId::Claude,
            vec![weekly(used, Some(reset))],
            t,
        ))]
    }

    #[test]
    fn level_critical_enters_at_90_holds_at_88_leaves_at_87_and_a_reset_clears_it() {
        let settings = settings_with(&[ProviderId::Claude]);
        let k = key(ProviderId::Claude, "weekly");
        let reset = at(10, 12, 9, 0);
        let mut memory = EvalMemory::default();
        let h = History::new();
        let run = |used: f64, reset: Timestamp, t: Timestamp, memory: &mut EvalMemory| {
            evaluate(&weekly_at(used, reset, t), &settings, &h, t, memory)
        };

        let e = run(0.80, reset, now(), &mut memory);
        assert_eq!(e.assessment(&k).expect("a").1.severity, Some(Severity::Ok));
        let e = run(0.90, reset, now() + mins(5), &mut memory);
        assert_eq!(
            e.assessment(&k).expect("a").1.state,
            RowState::CriticalLevel
        );
        assert!(e.changes.contains(&LiveChange::SeverityRose(k.clone())));
        let e = run(0.88, reset, now() + mins(10), &mut memory);
        assert_eq!(
            e.assessment(&k).expect("a").1.state,
            RowState::CriticalLevel
        );
        assert!(e.changes.is_empty(), "no announcement for a value change");
        let e = run(0.87, reset, now() + mins(15), &mut memory);
        assert_eq!(e.assessment(&k).expect("a").1.state, RowState::Ok);

        // Back to critical, then the window resets: memory cleared, the reset is announced.
        run(0.95, reset, now() + mins(20), &mut memory);
        let after = at(10, 12, 9, 5);
        let e = run(0.88, reset, after, &mut memory);
        assert_eq!(e.assessment(&k).expect("a").1.state, RowState::JustReset);
        assert!(e.changes.contains(&LiveChange::WindowReset(k.clone())));
        // The first reading of the new window at 88 % is not critical (memory was cleared).
        let next = at(10, 19, 9, 0);
        let e = run(0.88, next, at(10, 12, 9, 10), &mut memory);
        assert_eq!(e.assessment(&k).expect("a").1.state, RowState::Ok);
        assert!(
            !e.changes.contains(&LiveChange::WindowReset(k.clone())),
            "same instance as R'"
        );
    }

    #[test]
    fn freshness_changes_are_announced_once() {
        let settings = settings_with(&[ProviderId::Claude]);
        let read = now();
        let snap = snapshot(
            ProviderId::Claude,
            vec![weekly(0.5, Some(at(10, 12, 9, 0)))],
            read,
        );
        let mut memory = EvalMemory::default();
        let h = History::new();
        let e = evaluate(&[Svc::new(snap.clone())], &settings, &h, now(), &mut memory);
        assert!(e.changes.is_empty());
        let later = now() + mins(16);
        let e = evaluate(&[Svc::new(snap.clone())], &settings, &h, later, &mut memory);
        let source = e.services[0].source.clone();
        assert_eq!(e.changes, [LiveChange::FreshnessChanged(source)]);
        let locale = gb();
        let model = build_readout(&e, &build_cx(later, &locale, &settings));
        assert!(model.uia.announce);
        let e = evaluate(
            &[Svc::new(snap)],
            &settings,
            &h,
            later + mins(1),
            &mut memory,
        );
        assert!(e.changes.is_empty());
    }

    #[test]
    fn warning_turns_critical_by_runway_on_the_minute_tick() {
        let settings = settings_with(&[ProviderId::Claude]);
        let r = at(10, 10, 16, 30);
        let k = key(ProviderId::Claude, "five_hour");
        let tn = at(10, 10, 14, 20);
        // 30 min apart, +.15: rate .3/h, u .825 → t_out 14:55, 35 min away at 14:20.
        let h = five_hour_history(&[(at(10, 10, 13, 50), 0.675), (tn, 0.825)], r);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(0.825, Some(r))], tn);
        let mut memory = EvalMemory::default();
        let services = [Svc::new(snap)];
        let e = evaluate(&services, &settings, &h, tn, &mut memory);
        assert_eq!(e.assessment(&k).expect("a").1.state, RowState::Warning);
        let locale = gb();
        let model = build_readout(&e, &build_cx(tn, &locale, &settings));
        assert_eq!(cell_texts(&model, 0), ["[glyph]", "35 min left"]);
        // Five minutes later, no new reading: N = 30 → CRITICAL by runway.
        let tick = tn + mins(5);
        let e = evaluate(&services, &settings, &h, tick, &mut memory);
        assert_eq!(
            e.assessment(&k).expect("a").1.state,
            RowState::CriticalRunway
        );
        assert!(e.changes.contains(&LiveChange::SeverityRose(k)));
        let model = build_readout(&e, &build_cx(tick, &locale, &settings));
        assert_eq!(cell_texts(&model, 0), ["[glyph]", "30 min left"]);
        assert_eq!(compact_texts(&model, 0), ["[glyph]", "30 min"]);
    }
}

// ---- hover card --------------------------------------------------------------------------------

mod hover_card {
    use super::*;

    #[test]
    fn ok_card_with_also_tracked() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Codex, ProviderId::Meshy]);
        let r5 = at(10, 10, 14, 56);
        let mut h = five_hour_history(&[(at(10, 10, 12, 12), 0.375), (now(), 0.42)], r5);
        h.insert(key(ProviderId::Meshy, "credits"), meshy_history(1240.0));
        let services = [
            Svc::new(snapshot(
                ProviderId::Claude,
                vec![
                    five_hour(0.42, Some(r5)),
                    weekly(0.61, Some(at(10, 12, 9, 0))),
                ],
                now(),
            )),
            Svc::new(snapshot(
                ProviderId::Codex,
                vec![codex_weekly(0.10, Some(at(10, 16, 14, 26)))],
                now() - mins(1),
            )),
            Svc::new(meshy(1240.0, now())),
        ];
        let eval = eval_once(&services, &settings, &h, now());
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.glyph, Some(SeverityGlyph::Success));
        assert_eq!(card.header.title, "You'll make it to both resets");
        assert_eq!(card.services.len(), 1);
        assert_eq!(card.services[0].name, "Claude");
        assert_eq!(card.services[0].age, "Updated 1 min ago");
        let CardMeter::Window(five) = &card.services[0].meters[0] else {
            panic!("window expected");
        };
        assert_eq!(five.name, "5-hour session");
        assert_eq!(five.value, "42% used");
        assert!(!five.value_semibold);
        assert_eq!(five.elapsed_line, "55% of time gone");
        assert_eq!(five.forecast.as_deref(), Some("on course for 62% at reset"));
        assert_eq!(five.reset_line, "Resets 14:56");
        assert_eq!(five.reset_in.as_deref(), Some("in 2 h 14 min"));
        let CardMeter::Window(week) = &card.services[0].meters[1] else {
            panic!("window expected");
        };
        assert_eq!(week.reset_line, "Resets Mon 09:00");
        assert_eq!(week.reset_in.as_deref(), Some("in 1 d 20 h"));
        assert_eq!(week.forecast.as_deref(), Some("no forecast yet"));

        let also = card.also.as_ref().expect("also tracked");
        assert_eq!(also.header, "Also tracked");
        let labels: Vec<&str> = also.lines.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["Codex weekly", "Meshy credits"]);
        assert_eq!(also.lines[0].value, "10%");
        assert_eq!(also.lines[0].time.text, "Fri");
        assert_eq!(also.lines[1].value, "1,240");
        assert_eq!(also.lines[1].time.text, "≈ 9 days");
        assert_eq!(also.more, None);
        assert!(card.uia_name.starts_with("You'll make it to both resets "));
        assert!(card.sentence.contains("will be left when they reset"));
    }

    #[test]
    fn warning_and_critical_titles_and_sentences() {
        let settings = settings_with(&[ProviderId::Claude]);
        let r = at(10, 10, 16, 30);
        let locale = gb();

        let tn = at(10, 10, 13, 45);
        let h = five_hour_history(&[(at(10, 10, 13, 15), 0.71), (tn, 0.78)], r);
        let snap = snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.78, Some(r)),
                weekly(0.20, Some(at(10, 12, 9, 0))),
            ],
            tn,
        );
        let eval = eval_once(&[Svc::new(snap)], &settings, &h, tn);
        let cx = build_cx(tn, &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.glyph, Some(SeverityGlyph::Warning));
        assert_eq!(card.header.title, "5-hour limit runs out at 15:19");
        assert_eq!(
            card.sentence,
            "You're using it faster than time passes: 78% used with 45% of the window gone. At \
             the pace of the last 30 min it runs out 1 h 11 min before the 16:30 reset. The \
             weekly limit is fine."
        );
        let CardMeter::Window(w) = &card.services[0].meters[0] else {
            panic!("window expected");
        };
        assert_eq!(w.forecast.as_deref(), Some("runs out ≈ 15:19"));
        assert!(w.forecast_semibold && w.value_semibold);

        let tn = at(10, 10, 14, 50);
        let h = five_hour_history(&[(at(10, 10, 14, 20), 0.79), (tn, 0.94)], r);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(0.94, Some(r))], tn);
        let eval = eval_once(&[Svc::new(snap)], &settings, &h, tn);
        let cx = build_cx(tn, &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.glyph, Some(SeverityGlyph::Critical));
        assert_eq!(card.header.title, "5-hour limit runs out in 12 min");
        assert_eq!(
            card.sentence,
            "94% used. At the pace of the last 30 min it is empty at about 15:02, 1 h 28 min \
             before the reset at 16:30."
        );

        let tn = at(10, 10, 14, 50);
        let snap = snapshot(ProviderId::Claude, vec![five_hour(1.0, Some(r))], tn);
        let mut h = five_hour_history(&[(at(10, 10, 14, 40), 0.98), (at(10, 10, 14, 45), 1.0)], r);
        h.get_mut(&key(ProviderId::Claude, "five_hour"))
            .expect("history")
            .push(Sample::window(tn, 1.0, Some(r)));
        let eval = eval_once(&[Svc::new(snap)], &settings, &h, tn);
        let cx = build_cx(tn, &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.title, "5-hour limit is used up until 16:30");
        assert_eq!(
            card.sentence,
            "Used up since 14:45. It is back at 16:30, in 1 h 40 min."
        );
    }

    #[test]
    fn stale_card_drops_an_unknown_next_try() {
        let settings = settings_with(&[ProviderId::Claude]);
        let read = now() - mins(47);
        let snap = snapshot(
            ProviderId::Claude,
            vec![weekly(0.61, Some(at(10, 12, 9, 0)))],
            read,
        );
        let status = ServiceStatus {
            last_failure: Some((now(), FailureKind::Offline)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[Svc::new(snap.clone()).status(status.clone())],
            &settings,
            &History::new(),
            now(),
        );
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.glyph, Some(SeverityGlyph::Stale));
        assert_eq!(card.header.title, "No new reading from Claude for 47 min");
        assert_eq!(
            card.sentence,
            "Lastdrop is offline, so these are the numbers from 11:55. Bars are dashed and frozen \
             at that moment until a new reading arrives."
        );
        assert_eq!(card.services[0].age, "Last reading 11:55");
        let CardMeter::Window(w) = &card.services[0].meters[0] else {
            panic!("window expected");
        };
        assert_eq!(w.forecast, None);
        assert!(w.elapsed_line.ends_with("of time gone at 11:55"));

        let with_next = ServiceStatus {
            next_try: Some(at(10, 10, 13, 30)),
            last_failure: Some((now(), FailureKind::NoData)),
            failure_detail: Some("HTTP 503".to_owned()),
            ..status
        };
        let eval = eval_once(
            &[Svc::new(snap).status(with_next)],
            &settings,
            &History::new(),
            now(),
        );
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(
            card.sentence,
            "Claude isn't answering (HTTP 503), so these are the numbers from 11:55. Next try at \
             13:30."
        );
    }

    #[test]
    fn tray_anchor_with_the_readout_hidden_shows_the_winner() {
        let settings = settings_with(&[ProviderId::Claude]);
        let snap = snapshot(
            ProviderId::Claude,
            vec![
                five_hour(0.20, Some(at(10, 10, 14, 56))),
                weekly(0.61, Some(at(10, 12, 9, 0))),
            ],
            now(),
        );
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let card = build_hover_card(
            &eval,
            HoverAnchor::TrayIcon {
                readout_visible: false,
            },
            &[],
            &cx,
        );
        assert_eq!(card.services[0].meters.len(), 1);
        assert_eq!(card.header.title, "You'll make it to the reset");
        let also = card.also.expect("the 5-hour is also tracked");
        assert_eq!(also.lines[0].label, "Claude 5-hour");
        assert_eq!(also.lines[0].time.text, "14:56");
    }
}

// ---- tray --------------------------------------------------------------------------------------

mod tray {
    use super::*;

    #[test]
    fn tooltips_of_r9_3() {
        let settings = settings_with(&[ProviderId::Claude]);
        let locale = gb();

        let snap = snapshot(
            ProviderId::Claude,
            vec![weekly(0.61, Some(at(10, 12, 9, 0)))],
            now(),
        );
        let eval = eval_once(&[Svc::new(snap.clone())], &settings, &History::new(), now());
        let tray = build_tray(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(
            tray.tooltip,
            "Lastdrop: Claude weekly 61% used, resets Mon 09:00"
        );
        match tray.icon {
            TrayIcon::Digits { digits, role, bar } => {
                assert_eq!((digits.as_str(), role), ("61", ColorRole::TextPrimary));
                assert!(bar.elapsed.is_some() && !bar.dashed);
            }
            other => panic!("digits expected, got {other:?}"),
        }

        let r = at(10, 10, 16, 30);
        let tn = at(10, 10, 14, 50);
        let h = five_hour_history(&[(at(10, 10, 14, 20), 0.79), (tn, 0.94)], r);
        let s2 = snapshot(ProviderId::Claude, vec![five_hour(0.94, Some(r))], tn);
        let eval = eval_once(&[Svc::new(s2)], &settings, &h, tn);
        let tray = build_tray(&eval, &build_cx(tn, &locale, &settings));
        assert_eq!(tray.tooltip, "Lastdrop: Claude 5-hour runs out in 12 min");
        assert_eq!(
            tray.icon,
            TrayIcon::CriticalTile {
                digits: "94".to_owned()
            }
        );

        let read = now() - mins(47);
        let s3 = Snapshot {
            measured_at: read,
            fetched_at: read,
            ..snap
        };
        let offline = ServiceStatus {
            last_failure: Some((now(), FailureKind::Offline)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[Svc::new(s3).status(offline)],
            &settings,
            &History::new(),
            now(),
        );
        let tray = build_tray(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(
            tray.tooltip,
            "Lastdrop: Claude offline, last reading 47 min ago"
        );
        match tray.icon {
            TrayIcon::Digits { role, bar, .. } => {
                assert_eq!(role, ColorRole::Stale);
                assert!(bar.dashed);
            }
            other => panic!("stale digits expected, got {other:?}"),
        }
    }

    #[test]
    fn drop_while_loading_or_without_services() {
        let settings = settings_with(&[ProviderId::Claude]);
        let locale = gb();
        let eval = eval_once(
            &[Svc::loading(ProviderId::Claude)],
            &settings,
            &History::new(),
            now(),
        );
        let tray = build_tray(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(tray.icon, TrayIcon::Drop);
        assert_eq!(tray.tooltip, "Lastdrop: reading usage…");
        let eval = eval_once(&[], &settings, &History::new(), now());
        let tray = build_tray(&eval, &build_cx(now(), &locale, &settings));
        assert_eq!(
            (tray.icon, tray.tooltip.as_str()),
            (TrayIcon::Drop, "Lastdrop")
        );
    }

    #[test]
    fn digits_never_show_100() {
        let settings = settings_with(&[ProviderId::Claude]);
        let locale = gb();
        let mut m = weekly(0.97, Some(at(10, 12, 9, 0)));
        m.limited = true;
        let eval = eval_once(
            &[Svc::new(snapshot(ProviderId::Claude, vec![m], now()))],
            &settings,
            &History::new(),
            now(),
        );
        assert_eq!(
            build_tray(&eval, &build_cx(now(), &locale, &settings)).icon,
            TrayIcon::ExhaustedTile
        );
        let level = weekly(0.995, Some(at(10, 12, 9, 0)));
        let eval = eval_once(
            &[Svc::new(snapshot(ProviderId::Claude, vec![level], now()))],
            &settings,
            &History::new(),
            now(),
        );
        assert_eq!(
            build_tray(&eval, &build_cx(now(), &locale, &settings)).icon,
            TrayIcon::CriticalTile {
                digits: "99".to_owned()
            }
        );
    }
}

// ---- flyout ------------------------------------------------------------------------------------

mod flyout {
    use super::*;

    #[test]
    fn verdicts_rows_and_cards() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Codex, ProviderId::Meshy]);
        // R§13's critical vector shifted to `now`: +.15 in 30 min, 94 % → empty at 12:54.
        let r = at(10, 10, 14, 30);
        let mut h = five_hour_history(&[(at(10, 10, 12, 12), 0.79), (now(), 0.94)], r);
        h.insert(key(ProviderId::Meshy, "credits"), meshy_history(620.0));
        let week_reset = at(10, 12, 9, 0);
        h.insert(
            key(ProviderId::Claude, "weekly"),
            vec![
                Sample::window(at(10, 10, 8, 0), 0.60, Some(week_reset)),
                Sample::window(now(), 0.61, Some(week_reset)),
            ],
        );
        let mut claude = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.94, Some(r)), weekly(0.61, Some(week_reset))],
            now(),
        );
        claude.account.plan = Some("Max".to_owned());
        let services = [
            Svc::new(claude),
            Svc::new(snapshot(
                ProviderId::Codex,
                vec![codex_weekly(0.10, Some(at(10, 16, 14, 26)))],
                now(),
            )),
            Svc::new(meshy(620.0, now())),
        ];
        let eval = eval_once(&services, &settings, &h, now());
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        let flyout = build_flyout(&eval, &h, false, &cx);
        assert_eq!(flyout.title, "Lastdrop");
        assert_eq!(
            flyout.verdict.text,
            "Claude 5-hour runs out in 12 min · 1 more"
        );
        assert_eq!(flyout.verdict.glyph, Some(SeverityGlyph::Critical));
        assert_eq!(flyout.cards.len(), 3);
        let claude = &flyout.cards[0];
        assert_eq!(claude.name, "Claude");
        assert_eq!(claude.plan.as_deref(), Some("Max"));
        assert_eq!(claude.age, "Just now");
        assert_eq!(claude.a11y, "Claude, Max plan, updated 1 minute ago");
        assert_eq!(claude.message, None);
        let CardBody::Meters(rows) = &claude.body else {
            panic!("meters expected");
        };
        assert_eq!(rows[0].value, "94%");
        assert_eq!(rows[0].right.kind, RightKind::Say);
        assert_eq!(rows[0].right.glyph, Some(SeverityGlyph::Critical));
        assert_eq!(rows[0].right.text, "12 min left");
        assert_eq!(
            rows[0].line.as_deref(),
            Some("Empty ≈ 12:54, 1 h 36 min before the 14:30 reset.")
        );
        assert_eq!(rows[0].tone, Tone::Critical);
        assert_eq!(rows[0].bar.fill, ColorRole::Critical);
        assert_eq!(rows[1].right.kind, RightKind::Reset);
        assert_eq!(rows[1].right.text, "Resets Mon 09:00");
        assert_eq!(rows[1].tone, Tone::Calm);
        assert_eq!(rows[1].bar.fill, ColorRole::AccentFill);
        assert_eq!(rows[1].pct, Some(61));
        // The sparkline follows the weekly window: +1 point today.
        assert_eq!(claude.spark.values[13].map(f64::round), Some(1.0));
        assert_eq!(
            claude.spark.a11y,
            "Weekly limit used per day, last 14 days: today 1 percent, highest 1 percent on \
             Saturday, average 1 percent."
        );

        let meshy_card = &flyout.cards[2];
        let CardBody::Balance(b) = &meshy_card.body else {
            panic!("balance expected");
        };
        assert_eq!(b.amount, "620");
        assert_eq!(b.unit, "credits");
        assert_eq!(b.forecast, "≈ 4 days left");
        assert_eq!(b.forecast_glyph, Some(SeverityGlyph::Warning));
        assert_eq!(b.spent, "81% of last top-up used · 136 a day");
        assert_eq!(b.until.as_deref(), Some("until ≈ Thu 15 Oct"));
        assert_eq!(
            b.a11y,
            "620 credits left, 81 percent of the last top-up used, about 4 days at 136 a day"
        );
        let spark = spark_values(
            &h,
            &key(ProviderId::Meshy, "credits"),
            SparkKind::Balance,
            now(),
            &zone(),
        );
        assert_eq!(meshy_card.spark.values, spark);
        assert_eq!(meshy_card.spark.floor, 3260.0 * 0.05);
        assert_eq!(flyout.footer_note, None);
        assert_eq!(flyout.focus_card, None);
    }

    #[test]
    fn all_ok_loading_and_paused() {
        let settings = settings_with(&[ProviderId::Claude]);
        let locale = gb();
        let snap = snapshot(
            ProviderId::Claude,
            vec![weekly(0.2, Some(at(10, 12, 9, 0)))],
            now(),
        );
        let eval = eval_once(&[Svc::new(snap.clone())], &settings, &History::new(), now());
        let flyout = build_flyout(
            &eval,
            &History::new(),
            false,
            &build_cx(now(), &locale, &settings),
        );
        assert_eq!(flyout.verdict.text, "All limits last until their reset");
        assert_eq!(flyout.verdict.glyph, None);
        assert_eq!(flyout.cards[0].age, "Just now");

        let eval = eval_once(
            &[Svc::loading(ProviderId::Claude)],
            &settings,
            &History::new(),
            now(),
        );
        let flyout = build_flyout(
            &eval,
            &History::new(),
            true,
            &build_cx(now(), &locale, &settings),
        );
        assert_eq!(flyout.verdict.text, "Reading usage…");
        assert_eq!(flyout.cards[0].age, "Reading…");
        assert!(flyout.refreshing);

        let mut paused_settings = settings.clone();
        paused_settings.pause = Some(Pause { until: None });
        let paused = ServiceStatus {
            paused: true,
            ..fresh(now())
        };
        let eval = eval_once(
            &[Svc::new(snap).status(paused)],
            &paused_settings,
            &History::new(),
            now(),
        );
        let flyout = build_flyout(
            &eval,
            &History::new(),
            false,
            &build_cx(now(), &locale, &paused_settings),
        );
        assert_eq!(flyout.verdict.text, "Updates paused");
        let message = flyout.cards[0].message.as_ref().expect("message");
        assert_eq!(message.kind, MessageKind::Paused);
        assert_eq!(message.lead, "Paused");
        assert_eq!(message.text, " · showing 12:42");
        let footer = flyout.footer_note.expect("footer");
        assert_eq!(
            (footer.text.as_str(), footer.link.label.as_str()),
            ("Updates paused", "Resume")
        );
    }

    #[test]
    fn sign_in_cards_are_focused_and_offline_cards_explain() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Codex]);
        let locale = gb();
        let read = now() - mins(47);
        let claude = snapshot(
            ProviderId::Claude,
            vec![weekly(0.61, Some(at(10, 12, 9, 0)))],
            read,
        );
        let codex = snapshot(
            ProviderId::Codex,
            vec![codex_weekly(0.1, Some(at(10, 16, 14, 26)))],
            read,
        );
        let offline = ServiceStatus {
            last_failure: Some((now(), FailureKind::Offline)),
            next_try: Some(at(10, 10, 13, 30)),
            ..fresh(read)
        };
        let sign_in = ServiceStatus {
            last_failure: Some((now(), FailureKind::Auth)),
            ..fresh(read)
        };
        let eval = eval_once(
            &[
                Svc::new(claude).status(offline),
                Svc::new(codex).status(sign_in),
            ],
            &settings,
            &History::new(),
            now(),
        );
        let flyout = build_flyout(
            &eval,
            &History::new(),
            false,
            &build_cx(now(), &locale, &settings),
        );
        let first = flyout.cards[0].message.as_ref().expect("offline message");
        assert_eq!(first.kind, MessageKind::Offline);
        assert_eq!(first.glyph, SeverityGlyph::Stale);
        assert_eq!(
            format!("{}{}", first.lead, first.text),
            "Offline · showing 11:55 · next try 13:30"
        );
        assert_eq!(
            flyout.cards[0].a11y,
            "Claude, updated 47 minutes ago, offline"
        );
        let second = flyout.cards[1].message.as_ref().expect("sign-in message");
        assert_eq!(second.kind, MessageKind::SignIn);
        assert_eq!(second.lead, "Sign-in expired.");
        assert_eq!(
            second.action.as_ref().map(|a| a.label.as_str()),
            Some("Open sign-in…")
        );
        assert_eq!(flyout.focus_card, Some(1));
        assert_eq!(
            flyout.verdict.text,
            "Claude: no reading for 47 min · 1 more"
        );
        let CardBody::Meters(rows) = &flyout.cards[0].body else {
            panic!("meters expected");
        };
        assert!(rows[0].value_dim);
        assert_eq!(rows[0].tone, Tone::Stale);
        assert!(flyout.cards[0].spark.dim);
    }

    #[test]
    fn first_fetch_failure_and_missing_api_key() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Meshy]);
        let locale = gb();
        let failed = Svc {
            fetching: false,
            status: ServiceStatus {
                last_failure: Some((now(), FailureKind::NoData)),
                failure_detail: Some("HTTP 503".to_owned()),
                ..ServiceStatus::default()
            },
            ..Svc::loading(ProviderId::Claude)
        };
        let no_key = Svc {
            fetching: false,
            status: ServiceStatus {
                last_failure: Some((now(), FailureKind::Auth)),
                ..ServiceStatus::default()
            },
            ..Svc::loading(ProviderId::Meshy)
        };
        let eval = eval_once(&[failed, no_key], &settings, &History::new(), now());
        let flyout = build_flyout(
            &eval,
            &History::new(),
            false,
            &build_cx(now(), &locale, &settings),
        );
        let m = flyout.cards[0].message.as_ref().expect("failed message");
        assert_eq!(m.kind, MessageKind::FirstFetchFailed);
        assert_eq!(
            format!("{}{}", m.lead, m.text),
            "Couldn't read Claude · HTTP 503"
        );
        assert_eq!(m.action.as_ref().map(|a| a.label.as_str()), Some("Retry"));
        let k = flyout.cards[1].message.as_ref().expect("api key message");
        assert_eq!(k.kind, MessageKind::ApiKey);
        assert_eq!(
            k.action.as_ref().map(|a| a.label.as_str()),
            Some("Enter key…")
        );
        assert_eq!(flyout.focus_card, Some(1));
    }
}

// ---- Regressions from the M1 review (spec lens) ------------------------------------------------

mod review {
    use super::*;
    use lastdrop_core::grammar::a11y::row_sentence;
    use lastdrop_core::stats::WindowAssessment;

    /// One service's readings evaluated in turn with one memory, each recorded in the history
    /// first (as the app does).
    struct Run {
        settings: Settings,
        history: History,
        memory: EvalMemory,
    }

    impl Run {
        fn new(settings: Settings) -> Run {
            Run {
                settings,
                history: History::new(),
                memory: EvalMemory::default(),
            }
        }

        fn read(&mut self, snap: Snapshot, now: Timestamp) -> Evaluation {
            for m in &snap.meters {
                self.history
                    .entry(key(snap.provider, &m.id.0))
                    .or_default()
                    .push(Sample::window(snap.measured_at, m.used, m.resets_at));
            }
            evaluate(
                &[Svc::new(snap)],
                &self.settings,
                &self.history,
                now,
                &mut self.memory,
            )
        }
    }

    fn five_key() -> MeterKey {
        key(ProviderId::Claude, MeterId::FIVE_HOUR)
    }

    fn window_of(eval: &Evaluation, k: &MeterKey) -> WindowAssessment {
        match &eval.assessment(k).expect("assessed").1.meter {
            AssessedMeter::Window(w) => w.clone(),
            other => panic!("window expected, got {other:?}"),
        }
    }

    #[test]
    fn losing_the_reset_time_is_not_a_window_reset() {
        let r = at(10, 10, 16, 30);
        let mut run = Run::new(settings_with(&[ProviderId::Claude]));
        let t1 = at(10, 10, 12, 40);
        let e1 = run.read(
            snapshot(ProviderId::Claude, vec![five_hour(0.91, Some(r))], t1),
            t1,
        );
        assert_eq!(
            e1.assessment(&five_key()).expect("a").1.state,
            RowState::CriticalLevel
        );

        // The OAuth source rests after an error; Claude Desktop's history (no reset times)
        // answers with a newer reading of the same window.
        let t2 = at(10, 10, 12, 45);
        let e2 = run.read(
            snapshot(ProviderId::Claude, vec![five_hour(0.89, None)], t2),
            t2,
        );
        let (_, a) = e2.assessment(&five_key()).expect("a");
        assert!(
            !e2.changes.contains(&LiveChange::WindowReset(five_key())),
            "no false reset: {:?}",
            e2.changes
        );
        assert_eq!(
            a.state,
            RowState::CriticalLevel,
            "89 % holds level CRITICAL"
        );
        assert_eq!(window_of(&e2, &five_key()).resets_at, Some(r));

        // R comes back: the instance and its history are intact.
        let t3 = at(10, 10, 12, 55);
        let e3 = run.read(
            snapshot(ProviderId::Claude, vec![five_hour(0.89, Some(r))], t3),
            t3,
        );
        assert_eq!(
            e3.assessment(&five_key()).expect("a").1.state,
            RowState::CriticalLevel
        );
        let f = window_of(&e3, &five_key()).forecast.expect("forecast");
        assert_eq!(
            f.span,
            mins(15),
            "the forecast reaches back to the first reading"
        );
        assert!(f.valid);
        assert!(!e3.changes.contains(&LiveChange::WindowReset(five_key())));

        // Without the history (memory only), the shown instance still carries the reset time.
        let mut memory = EvalMemory::default();
        let settings = settings_with(&[ProviderId::Claude]);
        let first = snapshot(ProviderId::Claude, vec![five_hour(0.91, Some(r))], t1);
        evaluate(
            &[Svc::new(first)],
            &settings,
            &History::new(),
            t1,
            &mut memory,
        );
        let second = snapshot(ProviderId::Claude, vec![five_hour(0.89, None)], t2);
        let eval = evaluate(
            &[Svc::new(second)],
            &settings,
            &History::new(),
            t2,
            &mut memory,
        );
        assert!(eval.changes.is_empty(), "{:?}", eval.changes);
        assert_eq!(window_of(&eval, &five_key()).resets_at, Some(r));

        // A reading without R taken after the reset belongs to a new (unknown) instance.
        let late = at(10, 10, 16, 40);
        let e4 = run.read(
            snapshot(ProviderId::Claude, vec![five_hour(0.05, None)], late),
            late,
        );
        assert_eq!(window_of(&e4, &five_key()).resets_at, None);
    }

    #[test]
    fn a_warning_held_past_its_run_out_shows_the_projection_not_a_time() {
        let settings = settings_with(&[ProviderId::Claude]);
        let r = at(10, 10, 16, 30);
        let mut run = Run::new(settings.clone());
        for (h, m, u) in [(13, 15, 0.71), (13, 45, 0.78)] {
            let t = at(10, 10, h, m);
            run.read(
                snapshot(ProviderId::Claude, vec![five_hour(u, Some(r))], t),
                t,
            );
        }
        // p 0.9725 (> 0.97 keeps the WARNING), t_out 16:54 is after the 16:30 reset.
        let t = at(10, 10, 14, 15);
        let eval = run.read(
            snapshot(ProviderId::Claude, vec![five_hour(0.815, Some(r))], t),
            t,
        );
        let (service, a) = eval.assessment(&five_key()).expect("a");
        assert_eq!(a.state, RowState::Warning);
        let f = window_of(&eval, &five_key()).forecast.expect("forecast");
        assert!(!f.runs_out && f.valid);

        let locale = gb();
        let cx = build_cx(t, &locale, &settings);
        let readout = build_readout(&eval, &cx);
        assert_eq!(cell_texts(&readout, 0), ["[glyph]", "82%"]);
        assert_eq!(compact_texts(&readout, 0), ["[glyph]", "82%"]);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(
            card.header.title,
            "5-hour limit is on course for 97% at the reset"
        );
        assert!(
            card.sentence
                .ends_with("it reaches about 97% by the 16:30 reset."),
            "{}",
            card.sentence
        );
        let CardMeter::Window(w) = &card.services[0].meters[0] else {
            panic!("window expected");
        };
        assert_eq!(w.forecast.as_deref(), Some("on course for 97% at reset"));
        let flyout = build_flyout(&eval, &run.history, false, &cx);
        assert_eq!(
            flyout.verdict.text,
            "Claude 5-hour is on course for 97% at the reset"
        );
        let CardBody::Meters(rows) = &flyout.cards[0].body else {
            panic!("meters expected");
        };
        assert_eq!(rows[0].right.text, "Resets 16:30");
        assert_eq!(
            rows[0].line.as_deref(),
            Some("On course for 97% at the 16:30 reset.")
        );
        let sentence = row_sentence(a, service, &text_cx(t, &locale));
        assert!(
            sentence.contains("warning, on course for 97 percent at the reset"),
            "{sentence}"
        );
        let tray = build_tray(&eval, &cx);
        let everything = format!(
            "{readout:?} {card:?} {flyout:?} {sentence} {}",
            tray.tooltip
        );
        assert!(!everything.contains("16:54"), "no run-out after the reset");
        assert!(!everything.contains("0 min before"));
    }

    #[test]
    fn a_reading_without_a_per_model_meter_keeps_the_widths() {
        let settings = settings_with(&[ProviderId::Claude]);
        let mut run = Run::new(settings.clone());
        let locale = gb();
        let fable = window_meter(
            "weekly:fable",
            MeterNames::weekly_model("Fable"),
            2,
            0.2,
            Some(at(10, 12, 9, 0)),
            WindowLen::Fixed(hours(7 * 24)),
            false,
        );
        let t1 = now();
        let with = run.read(
            snapshot(
                ProviderId::Claude,
                vec![
                    five_hour(0.3, Some(at(10, 10, 14, 0))),
                    weekly(0.5, Some(at(10, 12, 9, 0))),
                    fable,
                ],
                t1,
            ),
            t1,
        );
        let t2 = t1 + mins(5);
        let without = run.read(
            snapshot(
                ProviderId::Claude,
                vec![five_hour(0.31, Some(at(10, 10, 14, 0))), weekly(0.5, None)],
                t2,
            ),
            t2,
        );
        let w1 = build_readout(&with, &build_cx(t1, &locale, &settings)).widths;
        let w2 = build_readout(&without, &build_cx(t2, &locale, &settings)).widths;
        assert!(w1.labels.contains(&"Fable".to_owned()));
        assert_eq!(w1, w2, "R§6.3: widths never change with data");
    }

    #[test]
    fn an_ok_card_without_reset_times_claims_no_pace_or_forecast() {
        let settings = settings_with(&[ProviderId::Claude]);
        let locale = gb();
        let cx = build_cx(now(), &locale, &settings);
        // Claude Desktop's history alone: no reset times, so no `e`, no pace, no forecast.
        let snap = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.10, None), weekly(0.55, None)],
            now() - mins(2),
        );
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.title, "Within your limits");
        assert_eq!(card.sentence, "");
        let flyout = build_flyout(&eval, &History::new(), false, &cx);
        assert_eq!(flyout.verdict.text, "Within your limits");

        // One reading with a reset time: a pace, but no forecast yet.
        let snap = snapshot(
            ProviderId::Claude,
            vec![weekly(0.40, Some(at(10, 12, 9, 0)))],
            now(),
        );
        let eval = eval_once(&[Svc::new(snap)], &settings, &History::new(), now());
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.title, "You'll make it to the reset");
        assert_eq!(
            card.sentence,
            "It is being used more slowly than time passes."
        );
    }

    #[test]
    fn small_text_fixes() {
        let settings = settings_with(&[ProviderId::Claude]);
        // F§4.4: the flyout's `Back` uses the card time rule (12-hour locale: `4:30 PM`).
        let r = at(10, 10, 16, 30);
        let mut used_up = five_hour(1.0, Some(r));
        used_up.limited = true;
        let eval = eval_once(
            &[Svc::new(snapshot(ProviderId::Claude, vec![used_up], now()))],
            &settings,
            &History::new(),
            now(),
        );
        let locale = us();
        let flyout = build_flyout(
            &eval,
            &History::new(),
            false,
            &build_cx(now(), &locale, &settings),
        );
        let CardBody::Meters(rows) = &flyout.cards[0].body else {
            panic!("meters expected");
        };
        assert_eq!(rows[0].right.text, "Back 4:30 PM");

        // R§11: verdict first for a used-up meter without a reset time.
        let mut weekly_empty = weekly(1.0, None);
        weekly_empty.limited = true;
        let eval = eval_once(
            &[Svc::new(snapshot(
                ProviderId::Claude,
                vec![weekly_empty],
                now(),
            ))],
            &settings,
            &History::new(),
            now(),
        );
        let (service, a) = eval
            .assessment(&key(ProviderId::Claude, MeterId::WEEKLY))
            .expect("a");
        assert_eq!(
            row_sentence(a, service, &text_cx(now(), &locale)),
            "Claude weekly, all models: empty, 100 percent used, no reset time given."
        );

        // R§5.3: the card's runway never shows 0 once `t_out` has passed.
        let history =
            five_hour_history(&[(at(10, 10, 14, 20), 0.79), (at(10, 10, 14, 50), 0.94)], r);
        let snap = snapshot(
            ProviderId::Claude,
            vec![five_hour(0.94, Some(r))],
            at(10, 10, 14, 50),
        );
        let later = at(10, 10, 15, 5);
        let eval = eval_once(&[Svc::new(snap)], &settings, &history, later);
        let gb = gb();
        let cx = build_cx(later, &gb, &settings);
        let card = build_hover_card(&eval, HoverAnchor::Readout, &eval.readout_rows, &cx);
        assert_eq!(card.header.title, "5-hour limit runs out in 1 min");
    }
}
