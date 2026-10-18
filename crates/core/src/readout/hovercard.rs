//! The hover card: a tooltip-style card over the readout or the tray icon, the readout's legend
//! (R§8). Drawn by the Slint `HoverCard` window (crates/app/ui/hovercard.slint), whose structs
//! mirror these types.

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::GlyphKind;
use super::bar::{BarInput, bar_input};
use super::build::BuildContext;
use crate::color::ColorRole;
use crate::consts::{ALSO_TRACKED_MAX, CARD_FORECAST_MAX_PCT};
use crate::glyph::{Icon, ProviderGlyph, SeverityGlyph};
use crate::grammar::TextContext;
use crate::grammar::a11y::{balance_sentence, runout_point};
use crate::grammar::cells::{back_text, course_pct, pct, reason_text, resets_at, t_out};
use crate::grammar::numbers::{amount, balance_duration_words, percent, share_pct, unit_word};
use crate::grammar::strings::{Msg, Strings, capitalise, fill, text};
use crate::grammar::time::{
    age_long_min1, age_token, card_date, card_time, long_duration, round_min, runout_text,
    shown_age, shown_duration, time_token,
};
use crate::model::{MeterKey, SourceKey};
use crate::stats::{
    AssessedMeter, Assessment, BalanceAssessment, Candidate, Evaluation, Freshness, Pace,
    ResetView, RowState, ServiceEval, Severity, StaleReason, WindowAssessment, candidate, compare,
    runway_minutes,
};

/// Templates with a value Lastdrop may not know: the template without the sentence that holds
/// `placeholder` (`Next try at {next}.` when no attempt is planned).
fn without_sentence(template: &str, placeholder: &str) -> String {
    template
        .split_inclusive(". ")
        .filter(|s| !s.contains(placeholder))
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// The meter is in trouble: WARNING or worse from fresh data (values turn semibold, R§8.2).
fn in_trouble(a: &Assessment) -> bool {
    a.severity.is_some_and(|s| s >= Severity::Warning)
}

/// The window assessment of a meter.
pub(crate) fn window(a: &Assessment) -> Option<&WindowAssessment> {
    match &a.meter {
        AssessedMeter::Window(w) => Some(w),
        AssessedMeter::Balance(_) | AssessedMeter::Loading => None,
    }
}

/// The balance assessment of a meter.
pub(crate) fn balance(a: &Assessment) -> Option<&BalanceAssessment> {
    match &a.meter {
        AssessedMeter::Balance(b) => Some(b),
        AssessedMeter::Window(_) | AssessedMeter::Loading => None,
    }
}

/// The last good reading of a service.
pub(crate) fn last_reading(service: &ServiceEval) -> Option<Timestamp> {
    service.status.last_success.or(service.reading_at)
}

/// `{n}` minutes as a long duration (`30 min`, `16 h 48 min`).
fn long(d: SignedDuration, cx: &TextContext<'_>) -> String {
    long_duration(d, cx.strings)
}

/// How long before the reset a meter runs out, as shown: `round_min(R) − round_min(t_out)`.
pub(crate) fn before_reset(t_out: Timestamp, reset: Timestamp, cx: &TextContext<'_>) -> String {
    long(round_min(reset).duration_since(round_min(t_out)), cx)
}

/// `{window}` of the OK sentence: session, week or month by the window length.
fn window_word(w: &WindowAssessment, cx: &TextContext<'_>) -> String {
    let msg = match w.window.map(|l| l.as_hours()) {
        Some(h) if h > 8 * 24 => Msg::CardWindowMonth,
        Some(h) if h > 24 => Msg::CardWindowWeek,
        _ => Msg::CardWindowSession,
    };
    cx.strings.template(msg).to_owned()
}

/// The service section of the card (R§8.2).
fn card_service(
    service: &ServiceEval,
    meters: &[&Assessment],
    cx: &TextContext<'_>,
) -> CardService {
    let age = if service.loading {
        cx.strings.template(Msg::Loading).to_owned()
    } else {
        match (service.freshness, last_reading(service)) {
            (Freshness::Stale(_), Some(t)) => text(
                cx.strings,
                Msg::CardAgeStale,
                &[("time", &card_time(t, cx))],
            ),
            (Freshness::Fresh, Some(t)) => {
                let age = age_long_min1(shown_age(cx.now, t).as_mins(), cx.strings);
                text(cx.strings, Msg::CardAge, &[("age", &age)])
            }
            (_, None) => String::new(),
        }
    };
    CardService {
        glyph: service.source.provider.glyph(),
        name: service.display_name().into_owned(),
        plan: service.account.plan.clone(),
        age,
        meters: meters.iter().map(|a| card_meter(a, cx)).collect(),
    }
}

/// A meter block (R§8.2).
fn card_meter(a: &Assessment, cx: &TextContext<'_>) -> CardMeter {
    match &a.meter {
        AssessedMeter::Balance(b) => CardMeter::Balance(card_balance(a, b, cx)),
        AssessedMeter::Window(w) => CardMeter::Window(card_window(a, Some(w), cx)),
        AssessedMeter::Loading if a.is_balance() => CardMeter::Balance(CardBalanceMeter {
            name: a.names.name.clone(),
            value: cx.strings.template(Msg::NoReset).to_owned(),
            value_semibold: false,
            bar: bar_input(a, ColorRole::TextPrimary),
            left_line: String::new(),
            forecast: cx.strings.template(Msg::Loading).to_owned(),
            forecast_semibold: false,
            rate_line: None,
            until: None,
        }),
        AssessedMeter::Loading => CardMeter::Window(card_window(a, None, cx)),
    }
}

/// A window meter block (R§8.2): value, bar, tick line with the forecast, reset line.
fn card_window(
    a: &Assessment,
    w: Option<&WindowAssessment>,
    cx: &TextContext<'_>,
) -> CardWindowMeter {
    let bar = bar_input(a, ColorRole::TextPrimary);
    let Some(w) = w else {
        return CardWindowMeter {
            name: a.names.name.clone(),
            value: cx.strings.template(Msg::NoReset).to_owned(),
            value_semibold: false,
            bar,
            elapsed_line: String::new(),
            forecast: None,
            forecast_semibold: false,
            reset_line: cx.strings.template(Msg::Loading).to_owned(),
            reset_in: None,
        };
    };
    let s = cx.strings;
    let stale = matches!(a.state, RowState::Stale(_));
    let idle = a.state == RowState::Idle
        || (stale && w.resets_at.is_none() && w.used <= 0.0 && w.starts_on_use);
    let elapsed_line = if idle {
        s.template(Msg::CardLineNotStarted).to_owned()
    } else {
        match (w.elapsed, w.resets_at, w.window) {
            (Some(e), _, _) => {
                let e = percent(share_pct(e), cx.locale);
                if stale && w.reset == ResetView::Live {
                    let time = card_time(w.reading_at, cx);
                    text(s, Msg::CardLineElapsedStale, &[("e", &e), ("time", &time)])
                } else {
                    text(s, Msg::CardLineElapsed, &[("e", &e)])
                }
            }
            (None, Some(_), None) => s.template(Msg::CardLineNoLength).to_owned(),
            (None, _, _) => s.template(Msg::CardLineElapsedUnknown).to_owned(),
        }
    };
    let (forecast, forecast_semibold) = match a.state {
        RowState::Stale(_) | RowState::Idle | RowState::Loading | RowState::Ended => (None, false),
        RowState::Exhausted => (Some(s.template(Msg::CardForecastUsedUp).to_owned()), false),
        RowState::Warning | RowState::CriticalRunway => {
            // A held WARNING without a run-out before the reset states its projection.
            let words = match (t_out(a), course_pct(a)) {
                (Some(t), _) => Some(text(
                    s,
                    Msg::CardForecastRunsOut,
                    &[("time", &card_time(t, cx))],
                )),
                (None, Some(p)) => Some(text(
                    s,
                    Msg::CardForecastCourse,
                    &[("p", &percent(p, cx.locale))],
                )),
                (None, None) => None,
            };
            (words, true)
        }
        RowState::Ok
        | RowState::OkNoReset
        | RowState::OkBalance
        | RowState::CriticalLevel
        | RowState::JustReset => {
            let words = match (w.resets_at, w.forecast) {
                (None, _) => None,
                (Some(_), Some(f)) if f.valid && f.rate_per_hour <= 0.0 => {
                    Some(s.template(Msg::CardForecastLasts).to_owned())
                }
                (Some(_), Some(f)) if f.valid => {
                    let p = share_pct(f.projected).min(CARD_FORECAST_MAX_PCT);
                    let p = percent(p, cx.locale);
                    Some(text(s, Msg::CardForecastCourse, &[("p", &p)]))
                }
                (Some(_), _) => Some(s.template(Msg::CardForecastNone).to_owned()),
            };
            (words, false)
        }
    };
    let reset_line = match (w.reset, w.resets_at) {
        (ResetView::Ended { by }, _) => {
            text(s, Msg::CardLineEnded, &[("time", &card_time(by, cx))])
        }
        (ResetView::JustReset { previous } | ResetView::IdleAfterReset { previous }, _) => text(
            s,
            Msg::CardLineJustReset,
            &[("time", &card_time(previous, cx))],
        ),
        _ if idle => s.template(Msg::CardLineIdle).to_owned(),
        (ResetView::Live, Some(r)) => text(s, Msg::CardLineReset, &[("time", &card_time(r, cx))]),
        (ResetView::Live, None) => s.template(Msg::CardLineNoReset).to_owned(),
    };
    let reset_in = match (w.reset, w.resets_at) {
        (ResetView::Live, Some(r)) => Some(text(
            s,
            Msg::CardLineIn,
            &[("duration", &long(shown_duration(cx.now, r), cx))],
        )),
        _ => None,
    };
    CardWindowMeter {
        name: a.names.name.clone(),
        value: if a.state == RowState::Ended {
            s.template(Msg::NoReset).to_owned()
        } else {
            text(s, Msg::CardValue, &[("pct", &percent(w.pct, cx.locale))])
        },
        value_semibold: in_trouble(a),
        bar,
        elapsed_line,
        forecast,
        forecast_semibold,
        reset_line,
        reset_in,
    }
}

/// A balance meter block (R§8.2).
fn card_balance(a: &Assessment, b: &BalanceAssessment, cx: &TextContext<'_>) -> CardBalanceMeter {
    let s = cx.strings;
    let stats = &b.stats;
    let trouble = matches!(
        a.state,
        RowState::Warning | RowState::CriticalRunway | RowState::Exhausted
    );
    let forecast = if stats.exhausted {
        s.template(Msg::CardBalanceEmpty).to_owned()
    } else {
        match (stats.days_left, stats.t_out) {
            (Some(_), Some(t)) if trouble => {
                text(s, Msg::CardBalanceRunsOut, &[("time", &time_token(t, cx))])
            }
            (Some(_), _) => match balance_duration_words(stats, s) {
                Some(d) => text(s, Msg::CardBalanceLasts, &[("duration", &d)]),
                None => s.template(Msg::CardBalanceNoUse).to_owned(),
            },
            (None, _) if !stats.valid => s.template(Msg::CardBalanceNoHistory).to_owned(),
            (None, _) => s.template(Msg::CardBalanceNoUse).to_owned(),
        }
    };
    let has_forecast = stats.days_left.is_some();
    CardBalanceMeter {
        name: a.names.name.clone(),
        value: text(
            s,
            Msg::CardBalanceValue,
            &[("pct", &percent(b.pct, cx.locale))],
        ),
        value_semibold: in_trouble(a),
        bar: bar_input(a, ColorRole::TextPrimary),
        left_line: text(
            s,
            Msg::CardBalanceLeft,
            &[
                ("amount", &amount(b.amount, cx.locale)),
                ("ref", &amount(stats.reference, cx.locale)),
                // `1 of 100 credits left`: the unit counts the top-up.
                ("unit", &unit_word(&b.unit, stats.reference, s)),
            ],
        ),
        forecast,
        forecast_semibold: trouble,
        rate_line: has_forecast.then(|| {
            text(
                s,
                Msg::CardBalanceRate,
                &[("burn", &amount(stats.burn_per_day, cx.locale))],
            )
        }),
        until: stats
            .t_out
            .filter(|_| has_forecast)
            .map(|t| text(s, Msg::CardBalanceUntil, &[("date", &card_date(t, cx))])),
    }
}

/// The time column of an "Also tracked" line: the time part of the meter's two-row readout
/// cell (R§8.2).
fn also_time(a: &Assessment, service: &ServiceEval, cx: &TextContext<'_>) -> AlsoTime {
    let s = cx.strings;
    let dash = || AlsoTime {
        glyph: None,
        text: s.template(Msg::NoReset).to_owned(),
        semibold: false,
        role: ColorRole::TextSecondary,
    };
    let say = |glyph: SeverityGlyph, words: String| AlsoTime {
        glyph: Some(GlyphKind::Severity(glyph)),
        text: words,
        semibold: true,
        role: ColorRole::TextPrimary,
    };
    match a.state {
        RowState::Ok | RowState::JustReset | RowState::CriticalLevel => match resets_at(a) {
            Some(r) => AlsoTime {
                glyph: Some(GlyphKind::Icon(Icon::Reset)),
                text: time_token(r, cx),
                semibold: false,
                role: ColorRole::TextPrimary,
            },
            None => dash(),
        },
        RowState::OkNoReset | RowState::Idle | RowState::Loading => dash(),
        // The time part of the two-row cell: the stale status, or the reading's age.
        RowState::Ended => match service.freshness {
            Freshness::Stale(reason) => stale_also_time(reason, service, cx),
            Freshness::Fresh => AlsoTime {
                glyph: None,
                text: last_reading(service)
                    .map_or_else(|| s.template(Msg::NoReset).to_owned(), |t| age_token(t, cx)),
                semibold: false,
                role: ColorRole::TextSecondary,
            },
        },
        RowState::OkBalance => match balance(a).and_then(|b| balance_duration_words(&b.stats, s)) {
            Some(words) => AlsoTime {
                glyph: None,
                text: format!("{} {words}", s.template(Msg::BalanceApprox)),
                semibold: false,
                role: ColorRole::TextPrimary,
            },
            None => dash(),
        },
        // A held WARNING without a run-out before the reset: ▲ + `pct`, as the readout shows it.
        RowState::Warning => match t_out(a) {
            Some(t) => say(SeverityGlyph::Warning, runout_text(t, false, cx)),
            None => say(SeverityGlyph::Warning, percent(pct(a), cx.locale)),
        },
        RowState::CriticalRunway => match t_out(a) {
            Some(t) => say(SeverityGlyph::Critical, runout_text(t, true, cx)),
            None => dash(),
        },
        RowState::Exhausted => say(SeverityGlyph::Critical, back_text(a, cx)),
        RowState::Stale(reason) => stale_also_time(reason, service, cx),
    }
}

/// The time column of a STALE "Also tracked" line: `Sign in` / `Paused`, else the age.
fn stale_also_time(reason: StaleReason, service: &ServiceEval, cx: &TextContext<'_>) -> AlsoTime {
    match reason {
        StaleReason::SignIn => AlsoTime {
            glyph: None,
            text: reason_text(reason, cx),
            semibold: false,
            role: ColorRole::AccentText,
        },
        StaleReason::Paused => AlsoTime {
            glyph: None,
            text: reason_text(reason, cx),
            semibold: false,
            role: ColorRole::Stale,
        },
        StaleReason::Offline | StaleReason::NoData | StaleReason::OldReading => AlsoTime {
            glyph: None,
            text: last_reading(service).map_or_else(
                || cx.strings.template(Msg::NoReset).to_owned(),
                |t| age_token(t, cx),
            ),
            semibold: false,
            role: ColorRole::Stale,
        },
    }
}

/// "Also tracked" (R§8.2): every enabled meter that is not shown above, the 4 highest by rank
/// (balances at any severity, so ranked by their own class) in service/meter order, then
/// `+N more`.
fn also_tracked(
    eval: &Evaluation,
    shown: &[MeterKey],
    cx: &TextContext<'_>,
) -> Option<AlsoTracked> {
    let s = cx.strings;
    // Each with its rank candidate, built once rather than twice per comparison.
    let mut ranked: Vec<(Candidate, &ServiceEval, &Assessment)> = eval
        .services
        .iter()
        .flat_map(|svc| svc.meters.iter().map(move |a| (svc, a)))
        .filter(|(_, a)| !shown.contains(&a.key) && a.state != RowState::Loading)
        .map(|(svc, a)| (candidate(a), svc, a))
        .collect();
    if ranked.is_empty() {
        return None;
    }
    ranked.sort_by(|(a, ..), (b, ..)| compare(a, b));
    let mut others: Vec<(&ServiceEval, &Assessment)> =
        ranked.into_iter().map(|(_, svc, a)| (svc, a)).collect();
    let more = others.len().saturating_sub(ALSO_TRACKED_MAX);
    others.truncate(ALSO_TRACKED_MAX);
    others.sort_by_key(|(_, a)| (a.service_order, a.meter_order));
    let lines = others
        .iter()
        .map(|(service, a)| AlsoLine {
            glyph: service.source.provider.glyph(),
            label: capitalise(&text(
                s,
                Msg::CardAlsoLine,
                &[
                    ("service", &service.display_name()),
                    ("short_name", &a.names.short_name),
                ],
            )),
            value: match balance(a) {
                Some(b) => amount(b.amount, cx.locale),
                None if a.state == RowState::Ended => s.template(Msg::NoReset).to_owned(),
                None => percent(pct(a), cx.locale),
            },
            value_semibold: in_trouble(a),
            time: also_time(a, service, cx),
        })
        .collect();
    Some(AlsoTracked {
        header: s.template(Msg::CardAlso).to_owned(),
        lines,
        more: (more > 0).then(|| text(s, Msg::CardAlsoMore, &[("n", &more.to_string())])),
        more_count: more,
    })
}

/// The `{meter}` of a card title: the `short_name`, or the meter with its service
/// (`card.also.line`: `Claude weekly`, `veckogränsen för Claude`) when the card shows several
/// services. The title is capitalised as a whole ([`trouble_header`]).
fn title_meter(service: &ServiceEval, a: &Assessment, several: bool, s: &dyn Strings) -> String {
    if several {
        text(
            s,
            Msg::CardAlsoLine,
            &[
                ("service", &service.display_name()),
                ("short_name", &a.names.short_name),
            ],
        )
    } else {
        a.names.short_name.clone()
    }
}

/// The header and sentence of a stale service (Appendix B `card.title.stale` / `signin`,
/// `card.sentence.offline` / `nodata` / `signin` / `paused`).
fn stale_header(
    service: &ServiceEval,
    reason: StaleReason,
    cx: &BuildContext<'_>,
) -> (CardHeader, String) {
    let t = &cx.text;
    let s = t.strings;
    let svc = service.display_name();
    let shown_at = last_reading(service).map(|r| card_time(r, t));
    let time = shown_at.clone().unwrap_or_default();
    let next = service.status.next_try.map(|n| card_time(n, t));
    let with_next = |msg: Msg, args: &[(&str, &str)]| {
        let template = s.template(msg);
        match &next {
            Some(n) => {
                let mut all = args.to_vec();
                all.push(("next", n));
                fill(template, &all)
            }
            None => fill(&without_sentence(template, "{next}"), args),
        }
    };
    let api_key = service.source.provider.client_name().is_none();
    let title = match reason {
        StaleReason::SignIn if api_key => text(s, Msg::CardTitleApiKey, &[("service", &svc)]),
        StaleReason::SignIn => text(s, Msg::CardTitleSignIn, &[("service", &svc)]),
        StaleReason::Paused
        | StaleReason::Offline
        | StaleReason::NoData
        | StaleReason::OldReading => {
            let age = last_reading(service)
                .map(|r| long(shown_age(t.now, r), t))
                .unwrap_or_default();
            text(s, Msg::CardTitleStale, &[("service", &svc), ("age", &age)])
        }
    };
    let sentence = match reason {
        StaleReason::SignIn if api_key => text(s, Msg::CardSentenceApiKey, &[("service", &svc)]),
        StaleReason::SignIn => text(
            s,
            Msg::CardSentenceSignIn,
            &[
                ("service", &svc),
                (
                    "client",
                    service.source.provider.client_name().unwrap_or_default(),
                ),
            ],
        ),
        StaleReason::Offline => with_next(Msg::CardSentenceOffline, &[("time", &time)]),
        StaleReason::OldReading => with_next(
            Msg::CardSentenceOldReading,
            &[("service", &svc), ("time", &time)],
        ),
        StaleReason::NoData => {
            let template = match &service.status.failure_detail {
                Some(_) => s.template(Msg::CardSentenceNoData).to_owned(),
                None => s
                    .template(Msg::CardSentenceNoData)
                    .replace(" ({detail})", ""),
            };
            let detail = service.status.failure_detail.clone().unwrap_or_default();
            let base = [
                ("service", &*svc),
                ("detail", detail.as_str()),
                ("time", time.as_str()),
            ];
            match &next {
                Some(n) => {
                    let mut all = base.to_vec();
                    all.push(("next", n));
                    fill(&template, &all)
                }
                None => fill(&without_sentence(&template, "{next}"), &base),
            }
        }
        StaleReason::Paused => match cx.settings.pause.and_then(|p| p.until) {
            Some(until) => text(
                s,
                Msg::CardSentencePaused,
                &[("until", &card_time(until, t)), ("time", &time)],
            ),
            None => text(s, Msg::CardSentencePausedOpen, &[("time", &time)]),
        },
    };
    let glyph = if reason == StaleReason::SignIn {
        SeverityGlyph::Info
    } else {
        SeverityGlyph::Stale
    };
    (
        CardHeader {
            glyph: Some(glyph),
            title: capitalise(&title),
        },
        sentence,
    )
}

/// The OK sentence (Appendix B `card.sentence.ok`, with R§3.4's pace wording).
fn ok_sentence(shown: &[(&ServiceEval, &Assessment)], cx: &TextContext<'_>) -> String {
    let s = cx.strings;
    let windows: Vec<&WindowAssessment> = shown.iter().filter_map(|(_, a)| window(a)).collect();
    if windows.is_empty() {
        // Only balances: their own sentences.
        return shown
            .iter()
            .filter(|(_, a)| balance(a).is_some())
            .map(|(svc, a)| balance_sentence(a, svc, cx))
            .collect::<Vec<_>>()
            .join(" ");
    }
    // Only what is known is said: a pace needs `e` in every window (R§3.2, R§3.4), "at your
    // recent pace … left when it resets" needs a valid forecast in at least one window (R§3.5)
    // and speaks only of windows with a reset time.
    let paces: Option<Vec<Pace>> = windows.iter().map(|w| w.pace).collect();
    let forecasted = windows.iter().any(|w| w.forecast.is_some_and(|f| f.valid));
    let with_reset: Vec<&WindowAssessment> = windows
        .iter()
        .copied()
        .filter(|w| w.resets_at.is_some())
        .collect();
    // Share left at the reset of the tightest window: 100 − p (p = u without a valid forecast).
    let left = with_reset
        .iter()
        .map(|w| {
            let p = w
                .forecast
                .filter(|f| f.valid)
                .map_or(w.used, |f| f.projected.max(w.used));
            100u8.saturating_sub(share_pct(p).min(100))
        })
        .min()
        .filter(|_| forecasted)
        .map(|l| percent(l, cx.locale));
    match (windows.as_slice(), paces.as_deref(), left) {
        ([w], Some([pace]), Some(left)) => {
            let msg = match pace {
                Pace::About => Msg::CardSentenceOkOneAbout,
                Pace::Ahead => Msg::CardSentenceOkOneAhead,
                Pace::Under => Msg::CardSentenceOkOne,
            };
            text(s, msg, &[("left", &left), ("window", &window_word(w, cx))])
        }
        ([_], Some([pace]), None) => s
            .template(match pace {
                Pace::About => Msg::CardSentenceOkOnePaceAbout,
                Pace::Ahead => Msg::CardSentenceOkOnePaceAhead,
                Pace::Under => Msg::CardSentenceOkOnePaceUnder,
            })
            .to_owned(),
        (_, Some(paces), left) if paces.len() > 1 && paces.iter().all(|p| *p == Pace::Under) => {
            match left {
                Some(left) => text(s, Msg::CardSentenceOkMany, &[("left", &left)]),
                None => s.template(Msg::CardSentenceOkManyPace).to_owned(),
            }
        }
        (_, _, Some(left)) => match with_reset.as_slice() {
            [w] => text(
                s,
                Msg::CardSentenceOkOneLeft,
                &[("left", &left), ("window", &window_word(w, cx))],
            ),
            _ => text(s, Msg::CardSentenceOkManyMixed, &[("left", &left)]),
        },
        (_, _, None) => String::new(),
    }
}

/// The header and sentence when the worst shown meter is in trouble (R§8.2, Appendix B).
fn trouble_header(
    service: &ServiceEval,
    a: &Assessment,
    shown: &[(&ServiceEval, &Assessment)],
    several: bool,
    cx: &TextContext<'_>,
) -> (CardHeader, String) {
    let s = cx.strings;
    let meter = title_meter(service, a, several, s);
    let pct_text = percent(pct(a), cx.locale);
    let reset = resets_at(a);
    let w = window(a);
    let span = w
        .and_then(|w| w.forecast)
        .map(|f| long(f.trailing, cx))
        .unwrap_or_default();
    let is_balance = balance(a).is_some();
    let (glyph, title, sentence) = match a.state {
        RowState::Exhausted => {
            let title = match reset {
                Some(r) => text(
                    s,
                    Msg::CardTitleExhausted,
                    &[("meter", &meter), ("time", &card_time(r, cx))],
                ),
                None => text(
                    s,
                    Msg::CardTitleCriticalLevel,
                    &[("meter", &meter), ("pct", &pct_text)],
                ),
            };
            let sentence = match (reset, w) {
                (Some(r), Some(w)) => {
                    let args = [
                        ("time", card_time(r, cx)),
                        ("duration", long(shown_duration(cx.now, r), cx)),
                    ];
                    match w.exhausted_since {
                        Some(since) => {
                            let since = card_time(since, cx);
                            text(
                                s,
                                Msg::CardSentenceExhausted,
                                &[
                                    ("since", &since),
                                    ("time", &args[0].1),
                                    ("duration", &args[1].1),
                                ],
                            )
                        }
                        None => text(
                            s,
                            Msg::CardSentenceExhaustedNoSince,
                            &[("time", &args[0].1), ("duration", &args[1].1)],
                        ),
                    }
                }
                _ if is_balance => balance_sentence(a, service, cx),
                _ => String::new(),
            };
            (SeverityGlyph::Critical, title, sentence)
        }
        RowState::CriticalRunway => {
            // The runway as shown, `N` (R§3.5, R§5.3: never 0, `1 min` once `t_out` passed).
            let title = match t_out(a) {
                Some(t) => text(
                    s,
                    Msg::CardTitleCriticalRunway,
                    &[
                        ("meter", &meter),
                        (
                            "duration",
                            &long(SignedDuration::from_mins(runway_minutes(cx.now, t)), cx),
                        ),
                    ],
                ),
                None => text(
                    s,
                    Msg::CardTitleCriticalLevel,
                    &[("meter", &meter), ("pct", &pct_text)],
                ),
            };
            let sentence = match (t_out(a), reset) {
                (Some(t), Some(r)) if !is_balance => text(
                    s,
                    Msg::CardSentenceCritical,
                    &[
                        ("pct", &pct_text),
                        ("span", &span),
                        ("time", &card_time(t, cx)),
                        ("duration", &before_reset(t, r, cx)),
                        ("reset", &card_time(r, cx)),
                    ],
                ),
                _ => balance_sentence(a, service, cx),
            };
            (SeverityGlyph::Critical, title, sentence)
        }
        RowState::CriticalLevel => {
            let title = text(
                s,
                Msg::CardTitleCriticalLevel,
                &[("meter", &meter), ("pct", &pct_text)],
            );
            let sentence = reset
                .map(|r| text(s, Msg::CardSentenceLevel, &[("time", &card_time(r, cx))]))
                .unwrap_or_default();
            (SeverityGlyph::Critical, title, sentence)
        }
        _ => {
            // WARNING. Held by hysteresis without a run-out before the reset (R§3.7, R§5.3): the
            // projection instead of a run-out time.
            let course = match t_out(a) {
                Some(_) => None,
                None => course_pct(a).map(|p| percent(p, cx.locale)),
            };
            let time = t_out(a).map(|t| runout_point(a, t, cx)).unwrap_or_default();
            let title = match (&course, several) {
                (Some(p), true) => text(
                    s,
                    Msg::CardTitleWarningCourseService,
                    &[
                        ("service", &service.display_name()),
                        ("meter", &a.names.short_name),
                        ("p", p),
                    ],
                ),
                (Some(p), false) => text(
                    s,
                    Msg::CardTitleWarningCourse,
                    &[("meter", &meter), ("p", p)],
                ),
                (None, true) => text(
                    s,
                    Msg::CardTitleWarningService,
                    &[
                        ("service", &service.display_name()),
                        ("meter", &a.names.short_name),
                        ("time", &time),
                    ],
                ),
                (None, false) => text(
                    s,
                    Msg::CardTitleWarning,
                    &[("meter", &meter), ("time", &time)],
                ),
            };
            let e = w
                .and_then(|w| w.elapsed)
                .map(|e| percent(share_pct(e), cx.locale))
                .unwrap_or_default();
            let mut sentence = match (t_out(a), &course, reset, w) {
                (Some(t), _, Some(r), Some(_)) => text(
                    s,
                    Msg::CardSentenceWarning,
                    &[
                        ("pct", &pct_text),
                        ("e", &e),
                        ("span", &span),
                        ("duration", &before_reset(t, r, cx)),
                        ("time", &card_time(r, cx)),
                    ],
                ),
                (None, Some(p), Some(r), Some(_)) => text(
                    s,
                    Msg::CardSentenceWarningCourse,
                    &[
                        ("pct", &pct_text),
                        ("e", &e),
                        ("span", &span),
                        ("p", p),
                        ("time", &card_time(r, cx)),
                    ],
                ),
                _ => balance_sentence(a, service, cx),
            };
            if let Some((_, other)) = shown
                .iter()
                .find(|(_, o)| o.key != a.key && o.severity == Some(Severity::Ok))
            {
                sentence.push_str(&text(
                    s,
                    Msg::CardSentenceOtherFine,
                    &[("other", &other.names.short_name)],
                ));
            }
            (SeverityGlyph::Warning, title, sentence)
        }
    };
    (
        CardHeader {
            glyph: Some(glyph),
            title: capitalise(&title),
        },
        sentence,
    )
}

/// What the pointer rests on (R§8.2 "Which meters").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HoverAnchor {
    /// The readout: its rows, grouped by service in readout order, then "Also tracked".
    Readout,
    /// The tray icon. With the readout hidden: the winner in full, then "Also tracked"; with the
    /// readout visible: as over the readout.
    TrayIcon {
        /// The readout is currently drawn (not HIDDEN).
        readout_visible: bool,
    },
}

/// The card header (R§8.2): severity glyph 16 px (cell 16 × 20) + 8 + title (Text 14/20
/// semibold, TextPrimary). Verdict and sentence follow the worst row (EXHAUSTED > CRITICAL >
/// WARNING > STALE > OK).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardHeader {
    /// Success (OK), Warning, Critical (critical and exhausted), Stale, Info (sign-in); `None`
    /// while everything is LOADING.
    pub glyph: Option<SeverityGlyph>,
    /// `card.title.*` (Appendix B).
    pub title: String,
}

/// A service section (R§8.2): margin-top 12, divider, padding-top 10; service row 16 high.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardService {
    /// Provider glyph, 16 px.
    pub glyph: ProviderGlyph,
    /// Name, 12/16 semibold (`Claude`, M2 `Claude · Work`).
    pub name: String,
    /// Plan, 12/16 TextSecondary; `None` when unknown.
    pub plan: Option<String>,
    /// Right-aligned age, 12/16 TextSecondary: `Updated 1 min ago`; stale: `Last reading 12:42`.
    pub age: String,
    /// Meter blocks.
    pub meters: Vec<CardMeter>,
}

/// A meter block (R§8.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CardMeter {
    /// A window meter.
    Window(CardWindowMeter),
    /// A balance meter.
    Balance(CardBalanceMeter),
}

/// A window meter block (R§8.2): row 16 (name, value); bar 296 × row 11; tick line; reset line.
/// Every block shows both keys (tick key, reset key), every time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardWindowMeter {
    /// Meter `name`, 12/16 semibold, left.
    pub name: String,
    /// `42% used`, right; semibold when the meter is in trouble.
    pub value: String,
    /// `value` is semibold.
    pub value_semibold: bool,
    /// The bar (calm fill TextPrimary: the card is the readout's key).
    pub bar: BarInput,
    /// After the tick key: `55% of time gone` (stale: `… at 12:42`); IDLE `window not started`;
    /// `L` unknown `window length unknown`.
    pub elapsed_line: String,
    /// Right of the tick line: `on course for 78% at reset`, `lasts to the reset`,
    /// `no forecast yet`, `runs out ≈ 15:20`, `used up`; `None` for STALE and IDLE.
    pub forecast: Option<String>,
    /// `forecast` is semibold (WARNING or CRITICAL).
    pub forecast_semibold: bool,
    /// After the reset key: `Resets 14:56`; IDLE `Starts with your next use`; `R` none
    /// `No reset time given`; JUST_RESET `Reset at 09:00 · waiting for the first reading`.
    pub reset_line: String,
    /// Right of the reset line: `in 2 h 14 min`; `None` when there is no reset time.
    pub reset_in: Option<String>,
}

/// A balance meter block (R§8.2): row 16; bar 296 without tick; amount line; rate line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardBalanceMeter {
    /// `Credits`, 12/16 semibold.
    pub name: String,
    /// `62% of top-up used`; semibold in trouble.
    pub value: String,
    /// `value` is semibold.
    pub value_semibold: bool,
    /// The bar, no tick.
    pub bar: BarInput,
    /// `1,240 of 3,260 credits left` (TextSecondary).
    pub left_line: String,
    /// `lasts ≈ 9 days` / `runs out ≈ Thu` / `empty` / `no use in 7 days` /
    /// `not enough history yet`.
    pub forecast: String,
    /// `forecast` is semibold (WARNING/CRITICAL).
    pub forecast_semibold: bool,
    /// `136 a day, last 7 days`; `None` without a forecast.
    pub rate_line: Option<String>,
    /// `until ≈ Mon 19 Oct`; `None` without a forecast.
    pub until: Option<String>,
}

/// One "Also tracked" line, 20 px (R§8.2): glyph 12, 6, `{service} {short_name}`, value right,
/// then the time column (as wide as its widest entry, at least 56 px).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlsoLine {
    /// Provider glyph, 12 px.
    pub glyph: ProviderGlyph,
    /// `Claude 5-hour`, `Codex weekly`, `Meshy credits`.
    pub label: String,
    /// `42%`; balance: the amount `1,240`.
    pub value: String,
    /// `value` is semibold (in trouble).
    pub value_semibold: bool,
    /// The time part of the meter's two-row readout cell.
    pub time: AlsoTime,
}

/// The time column of an "Also tracked" line (R§8.2): one optional glyph and one text, so the
/// Slint `AlsoLineData` maps 1:1 (`time-glyph`, `time-text`, `time-semibold`, `time-dim`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlsoTime {
    /// ↺ (`Icon::Reset`, TextSecondary) before a reset time; ▲/● before say-text; `None` for
    /// `≈ 9 days`, `–` and stale statuses.
    pub glyph: Option<GlyphKind>,
    /// `14:56`, `Out 15:20`, `12 min left`, `Back 16:30`, `≈ 9 days`, `–`, `47m ago`.
    pub text: String,
    /// Say-text (after ▲/●) is semibold, as in the readout (R§6.5).
    pub semibold: bool,
    /// TextPrimary; TextSecondary for `–`; Stale for a stale service's status; AccentText for
    /// `Sign in`.
    pub role: ColorRole,
}

/// The "Also tracked" section (R§8.2): every enabled meter that is not a readout row; the 4
/// highest by rank (balances at any severity) in service/meter order, then `+N more`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlsoTracked {
    /// `Also tracked`.
    pub header: String,
    /// Up to `ALSO_TRACKED_MAX` lines.
    pub lines: Vec<AlsoLine>,
    /// `+N more`, when more meters exist.
    pub more: Option<String>,
    /// The `N` of `more` (0 without it), so a window that drops lines to fit (R§8.1) can count
    /// the dropped ones in.
    pub more_count: usize,
}

/// The hover card (R§8). Width 320, padding 12, height = content. When the content is taller
/// than the space, the window drops "Also tracked" lines from the bottom, then the sentence
/// (R§8.1); the model always carries everything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoverCardModel {
    /// Header.
    pub header: CardHeader,
    /// The sentence (Appendix B `card.sentence.*`), margin-top 4, 12/16, wraps.
    pub sentence: String,
    /// Service sections, readout order.
    pub services: Vec<CardService>,
    /// "Also tracked"; `None` when every enabled meter is already shown.
    pub also: Option<AlsoTracked>,
    /// UIA ToolTip name: title + sentence (R§11), raised as `ToolTipOpened`.
    pub uia_name: String,
}

/// Builds the hover card for `anchor` (R§8.2). `readout_rows` are the rows the readout currently
/// shows (from the last `PlacementChanged`: the compact form shows one).
pub fn build_hover_card(
    eval: &Evaluation,
    anchor: HoverAnchor,
    readout_rows: &[MeterKey],
    cx: &BuildContext<'_>,
) -> HoverCardModel {
    let t = &cx.text;
    let s = t.strings;
    let keys: Vec<MeterKey> = match anchor {
        HoverAnchor::TrayIcon {
            readout_visible: false,
        } => eval.winner.iter().cloned().collect(),
        HoverAnchor::Readout | HoverAnchor::TrayIcon { .. } if readout_rows.is_empty() => {
            eval.winner.iter().cloned().collect()
        }
        HoverAnchor::Readout | HoverAnchor::TrayIcon { .. } => readout_rows.to_vec(),
    };
    let shown: Vec<(&ServiceEval, &Assessment)> =
        keys.iter().filter_map(|k| eval.assessment(k)).collect();

    // Service sections in readout order, one header per service.
    let mut sources: Vec<&SourceKey> = Vec::new();
    for (service, _) in &shown {
        if !sources.contains(&&service.source) {
            sources.push(&service.source);
        }
    }
    let services: Vec<CardService> = sources
        .iter()
        .filter_map(|source| {
            let meters: Vec<&Assessment> = shown
                .iter()
                .filter(|(svc, _)| &&svc.source == source)
                .map(|(_, a)| *a)
                .collect();
            let (service, _) = shown.iter().find(|(svc, _)| &&svc.source == source)?;
            Some(card_service(service, &meters, t))
        })
        .collect();
    let several = sources.len() > 1;

    // The worst shown row decides the header and the sentence (EXHAUSTED > CRITICAL > WARNING >
    // STALE > OK; ties by rank).
    let rank_of = |k: &MeterKey| {
        eval.ranked
            .iter()
            .position(|r| r == k)
            .unwrap_or(usize::MAX)
    };
    let worst = shown.iter().min_by(|(_, a), (_, b)| {
        b.class
            .cmp(&a.class)
            .then(rank_of(&a.key).cmp(&rank_of(&b.key)))
    });
    let (header, sentence) = match worst {
        None => (
            CardHeader {
                glyph: None,
                title: s.template(Msg::FlyoutVerdictLoading).to_owned(),
            },
            String::new(),
        ),
        Some((service, a)) => match a.state {
            RowState::Loading => (
                CardHeader {
                    glyph: None,
                    title: s.template(Msg::FlyoutVerdictLoading).to_owned(),
                },
                String::new(),
            ),
            RowState::Stale(reason) => stale_header(service, reason, cx),
            // Every shown row is ENDED (or LOADING): the service's staleness, else nothing yet.
            RowState::Ended => match service.freshness {
                Freshness::Stale(reason) => stale_header(service, reason, cx),
                Freshness::Fresh => (
                    CardHeader {
                        glyph: None,
                        title: s.template(Msg::FlyoutVerdictLoading).to_owned(),
                    },
                    String::new(),
                ),
            },
            RowState::Exhausted
            | RowState::CriticalRunway
            | RowState::CriticalLevel
            | RowState::Warning => trouble_header(service, a, &shown, several, t),
            RowState::Ok
            | RowState::OkNoReset
            | RowState::Idle
            | RowState::OkBalance
            | RowState::JustReset => {
                // LOADING and ENDED rows tell nothing about now: neither title nor sentence
                // speaks of them.
                let known: Vec<(&ServiceEval, &Assessment)> = shown
                    .iter()
                    .filter(|(_, a)| !matches!(a.state, RowState::Loading | RowState::Ended))
                    .copied()
                    .collect();
                let counted = known.len();
                // "You'll make it to the reset" needs a reset: windows without any reset time
                // (Claude Desktop's history alone) get a title that claims none.
                let windows: Vec<&WindowAssessment> =
                    known.iter().filter_map(|(_, a)| window(a)).collect();
                let no_reset = !windows.is_empty()
                    && windows.iter().all(|w| w.resets_at.is_none())
                    && known.iter().all(|(_, a)| balance(a).is_none());
                let msg = match counted {
                    _ if no_reset => Msg::CardTitleOkNoReset,
                    0 | 1 => Msg::CardTitleOkOne,
                    2 => Msg::CardTitleOkTwo,
                    _ => Msg::CardTitleOkMany,
                };
                (
                    CardHeader {
                        glyph: Some(SeverityGlyph::Success),
                        title: s.template(msg).to_owned(),
                    },
                    ok_sentence(&known, t),
                )
            }
        },
    };
    let uia_name = format!("{} {sentence}", header.title).trim_end().to_owned();
    HoverCardModel {
        header,
        sentence,
        services,
        also: also_tracked(eval, &keys, t),
        uia_name,
    }
}
