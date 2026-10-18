//! UI Automation sentences (R§11) and the tray tooltip (R§9.3).
//!
//! Row sentence grammar: `<Service> <meter name>: <verdict>, <facts>.` — verdict first; full
//! times with AM/PM where the locale uses it ([`super::time::spoken_time`]). `{meter}` here is
//! the meter's **spoken name**: its `name` with ` · ` read as `, ` and the first letter
//! lower-cased unless it is a digit or the language capitalises nouns
//! ([`Strings::lowercase_names`]) — `5-hour session`, `weekly, all models`, `weekly, Fable`
//! (R§11's examples; a screen reader would read `·` aloud). Percentages are spoken as
//! `{n} percent` with a bare integer.

use jiff::Timestamp;

use super::TextContext;
use super::cells::{course_pct, pct, resets_at, t_out};
use super::numbers::{amount, balance_left, percent, unit_word};
use super::strings::{Msg, Strings, text};
use super::time::{
    CountdownForm, card_time, countdown, long_duration, shown_age, spoken_duration, spoken_point,
    spoken_time, time_token,
};
use crate::consts::TRAY_TIP_MAX_CHARS;
use crate::stats::{
    AssessedMeter, Assessment, Evaluation, Freshness, ResetView, RowState, ServiceEval,
    StaleReason, runway_minutes,
};

/// The spoken name of a meter (module docs): `·` read as a comma, the first letter lower-cased
/// unless it is a digit or the language keeps it (`Weekly · all models` → `weekly, all models`).
pub(crate) fn spoken_name(name: &str, strings: &dyn Strings) -> String {
    let joined = name.replace(" · ", ", ").replace('·', ",");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) if strings.lowercase_names() && !first.is_ascii_digit() => {
            first.to_lowercase().chain(chars).collect()
        }
        _ => joined,
    }
}

/// The first sentence of `s` (through its first `. `), for templates whose tail has nothing to
/// list.
fn first_sentence(s: &str) -> &str {
    s.find(". ").map_or(s, |i| &s[..=i])
}

/// The time a meter runs out at in running text: the card rule for windows (`3:20 PM`), the
/// readout token for balances (`Thu`: a balance forecast is days away and approximate).
pub(crate) fn runout_point(a: &Assessment, t: Timestamp, cx: &TextContext<'_>) -> String {
    match &a.meter {
        AssessedMeter::Balance(_) => time_token(t, cx),
        AssessedMeter::Window(_) | AssessedMeter::Loading => card_time(t, cx),
    }
}

/// The age of a service's last reading, as shown.
fn last_reading(service: &ServiceEval) -> Option<Timestamp> {
    service.status.last_success.or(service.reading_at)
}

/// A balance meter's sentence (R§11): `Meshy: 1,240 credits left, about 9 days at the current
/// rate.`
pub(crate) fn balance_sentence(
    a: &Assessment,
    service: &ServiceEval,
    cx: &TextContext<'_>,
) -> String {
    let svc = service.display_name();
    let AssessedMeter::Balance(b) = &a.meter else {
        return String::new();
    };
    let amount = amount(b.amount, cx.locale);
    let unit = unit_word(&b.unit, b.amount, cx.strings);
    let duration = balance_left(&b.stats, cx.now).map(|d| spoken_duration(d, cx.strings));
    match duration {
        Some(duration) => text(
            cx.strings,
            Msg::A11yBalance,
            &[
                ("service", &svc),
                ("amount", &amount),
                ("unit", &unit),
                ("duration", &duration),
            ],
        ),
        None => text(
            cx.strings,
            Msg::A11yBalanceNoForecast,
            &[("service", &svc), ("amount", &amount), ("unit", &unit)],
        ),
    }
}

/// The sentence of a row that is not stale; a stale row gets its short `{meter} {n} percent
/// used` form (callers describe stale services with [`service_sentence`]).
fn row_core(a: &Assessment, service: &ServiceEval, cx: &TextContext<'_>) -> String {
    let svc = service.display_name();
    let meter = spoken_name(&a.names.name, cx.strings);
    let n = pct(a).to_string();
    let base = [
        ("service", &*svc),
        ("meter", meter.as_str()),
        ("n", n.as_str()),
    ];
    let with = |msg: Msg, extra: &[(&str, &str)]| {
        let mut args: Vec<(&str, &str)> = base.to_vec();
        args.extend_from_slice(extra);
        text(cx.strings, msg, &args)
    };
    if matches!(a.meter, AssessedMeter::Balance(_)) && a.state != RowState::Loading {
        if let RowState::Stale(_) = a.state {
            return text(
                cx.strings,
                Msg::A11yStaleRow,
                &[("meter", &meter), ("n", &n)],
            );
        }
        return balance_sentence(a, service, cx);
    }
    let reset = resets_at(a).map(|r| spoken_time(r, cx));
    let no_reset = || with(Msg::A11yRowNoReset, &[]);
    match a.state {
        RowState::Loading => with(Msg::A11yRowLoading, &[]),
        RowState::Ended => match &a.meter {
            AssessedMeter::Window(w) => match w.reset {
                ResetView::Ended { by } => {
                    with(Msg::A11yRowEnded, &[("time", &spoken_point(by, cx))])
                }
                _ => no_reset(),
            },
            _ => no_reset(),
        },
        RowState::Stale(_) => text(
            cx.strings,
            Msg::A11yStaleRow,
            &[("meter", &meter), ("n", &n)],
        ),
        RowState::Idle => with(Msg::A11yRowIdle, &[]),
        RowState::OkNoReset | RowState::OkBalance => no_reset(),
        RowState::Ok | RowState::JustReset => match &reset {
            Some(r) => with(Msg::A11yRowOk, &[("reset", r)]),
            None => no_reset(),
        },
        RowState::Warning => match (t_out(a), course_pct(a), &reset) {
            (Some(t), _, Some(r)) => with(
                Msg::A11yRowWarning,
                &[("time", &spoken_point(t, cx)), ("reset", r)],
            ),
            // Held by hysteresis without a run-out before the reset (R§5.3).
            (None, Some(p), Some(r)) => with(
                Msg::A11yRowWarningCourse,
                &[("course", &p.to_string()), ("reset", r)],
            ),
            _ => no_reset(),
        },
        RowState::CriticalRunway => match (t_out(a), &reset) {
            (Some(t), Some(r)) => {
                let mins = runway_minutes(cx.now, t);
                let duration = spoken_duration(jiff::SignedDuration::from_mins(mins), cx.strings);
                with(Msg::A11yRowRunway, &[("duration", &duration), ("reset", r)])
            }
            _ => no_reset(),
        },
        RowState::CriticalLevel => match &reset {
            Some(r) => with(Msg::A11yRowLevel, &[("reset", r)]),
            None => no_reset(),
        },
        RowState::Exhausted => match &reset {
            Some(r) => with(Msg::A11yRowExhausted, &[("time", r)]),
            // Verdict first (R§11), as every row sentence.
            None => with(Msg::A11yRowExhaustedNoReset, &[]),
        },
    }
}

/// The sentence of one readout row / flyout meter row (R§11 examples):
///
/// - `Claude 5-hour session: on track, 42 percent used, resets at 2:56 PM.`
/// - `Claude 5-hour session: warning, runs out at about 3:20 PM, before the reset at 4:30 PM; 78 percent used.`
/// - `Claude 5-hour session: critical, runs out in about 12 minutes; 94 percent used; resets at 4:30 PM.`
/// - `Claude 5-hour session: critical, 92 percent used, lasts until the reset at 4:30 PM.`
/// - `Claude weekly, all models: critical, runs out in about 6 hours; 91 percent used; resets Monday 9:00 AM.`
/// - `Claude 5-hour session: empty, back at 4:30 PM.`
/// - `Claude 5-hour session: not started, 0 percent used, starts with your next message.`
/// - `Claude 5-hour session: ended by 5:42 PM, no newer reading.`
/// - `Meshy: 1,240 credits left, about 9 days at the current rate.`
///
/// A stale meter is described by its service's sentence ([`service_sentence`]); an ENDED one
/// has no percentage to list there and keeps its own sentence.
pub fn row_sentence(a: &Assessment, service: &ServiceEval, cx: &TextContext<'_>) -> String {
    match a.state {
        RowState::Stale(_) => service_sentence(service, &[a], cx),
        _ => row_core(a, service, cx),
    }
}

/// The sentence of a stale service (R§11):
///
/// - `Claude: offline, last reading 47 minutes ago. Then: 5-hour session 42 percent used, weekly 61 percent used.`
/// - `Claude: sign-in expired, last reading 47 minutes ago. Select to open Lastdrop and see how to sign in.`
///
/// `rows` are the service's meters to list after "Then:".
pub fn service_sentence(
    service: &ServiceEval,
    rows: &[&Assessment],
    cx: &TextContext<'_>,
) -> String {
    let Freshness::Stale(reason) = service.freshness else {
        // Not stale after all: the rows' own sentences.
        return rows
            .iter()
            .map(|a| row_core(a, service, cx))
            .collect::<Vec<_>>()
            .join(" ");
    };
    let svc = service.display_name();
    let age = last_reading(service).map_or_else(
        || cx.strings.template(Msg::NoReset).to_owned(),
        |t| spoken_duration(shown_age(cx.now, t), cx.strings),
    );
    // ENDED rows have no percentage to recall; they keep their own sentence.
    let list = rows
        .iter()
        .filter(|a| a.state != RowState::Ended)
        .map(|a| {
            let n = pct(a).to_string();
            text(
                cx.strings,
                Msg::A11yStaleRow,
                &[
                    ("meter", &spoken_name(&a.names.name, cx.strings)),
                    ("n", &n),
                ],
            )
        })
        .collect::<Vec<_>>()
        .join(cx.strings.template(Msg::A11yListSep));
    let msg = match reason {
        StaleReason::SignIn if service.source.provider.client_name().is_none() => {
            Msg::A11yServiceApiKey
        }
        StaleReason::SignIn => Msg::A11yServiceSignIn,
        StaleReason::Offline => Msg::A11yServiceOffline,
        StaleReason::NoData => Msg::A11yServiceNoData,
        StaleReason::OldReading => Msg::A11yServiceOldReading,
        StaleReason::Paused => Msg::A11yServicePaused,
    };
    let sentence = text(
        cx.strings,
        msg,
        &[("service", &svc), ("age", &age), ("rows", &list)],
    );
    if list.is_empty()
        && matches!(
            reason,
            StaleReason::Offline
                | StaleReason::NoData
                | StaleReason::OldReading
                | StaleReason::Paused
        )
    {
        first_sentence(&sentence).to_owned()
    } else {
        sentence
    }
}

/// The readout root's name: `Lastdrop. ` + the row sentences (R§11, ControlType Button).
pub fn root_name(sentences: &[String], cx: &TextContext<'_>) -> String {
    text(cx.strings, Msg::A11yRoot, &[("rows", &sentences.join(" "))])
        .trim_end()
        .to_owned()
}

/// The tray icon's `szTip`, ≤ 127 characters (`TRAY_TIP_MAX_CHARS`), also its accessible name
/// (R§9.3): `Lastdrop: Claude weekly 61% used, resets Mon 09:00` ·
/// `Lastdrop: Claude 5-hour runs out in 12 min` · `Lastdrop: Claude offline, last reading 47 min ago`.
/// Describes the winner (R§3.10).
pub fn tray_tooltip(eval: &Evaluation, cx: &TextContext<'_>) -> String {
    let nothing = || {
        let msg = if eval.services.iter().any(|s| s.loading) {
            Msg::TrayLoading
        } else {
            Msg::TrayNothing
        };
        cx.strings.template(msg).to_owned()
    };
    let Some((service, a)) = eval.winner.as_ref().and_then(|k| eval.assessment(k)) else {
        return nothing();
    };
    let svc = service.display_name();
    let pct_text = percent(pct(a), cx.locale);
    let args = |extra: &[(&'static str, String)]| {
        let mut v: Vec<(&str, String)> = vec![
            ("service", svc.clone().into_owned()),
            ("meter", a.names.short_name.clone()),
            ("pct", pct_text.clone()),
        ];
        v.extend(extra.iter().map(|(k, s)| (*k, s.clone())));
        v
    };
    let say = |msg: Msg, extra: &[(&'static str, String)]| {
        let owned = args(extra);
        let refs: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        text(cx.strings, msg, &refs)
    };
    let reset = resets_at(a);
    // A balance has its own texts: its name may be plural (`Meshy credits`).
    let is_balance = matches!(a.meter, AssessedMeter::Balance(_));
    // An ENDED winner (every candidate tells nothing now) is described by its service's
    // staleness, as a STALE row would be; fresh, there is nothing to say.
    let state = match (a.state, service.freshness) {
        (RowState::Ended, Freshness::Stale(reason)) => RowState::Stale(reason),
        (state, _) => state,
    };
    let tip = match state {
        RowState::Loading => cx.strings.template(Msg::TrayLoading).to_owned(),
        RowState::Ended => cx.strings.template(Msg::TrayNothing).to_owned(),
        RowState::Stale(StaleReason::SignIn) if service.source.provider.client_name().is_none() => {
            say(Msg::TrayApiKey, &[])
        }
        RowState::Stale(StaleReason::SignIn) => say(Msg::TraySignIn, &[]),
        RowState::Stale(StaleReason::Paused) => say(Msg::TrayPaused, &[]),
        RowState::Stale(reason) => {
            let age = last_reading(service).map_or_else(
                || cx.strings.template(Msg::NoReset).to_owned(),
                |t| long_duration(shown_age(cx.now, t), cx.strings),
            );
            let msg = match reason {
                StaleReason::Offline => Msg::TrayOffline,
                StaleReason::OldReading => Msg::TrayOldReading,
                _ => Msg::TrayNoData,
            };
            say(msg, &[("age", age)])
        }
        RowState::Ok | RowState::JustReset => match reset {
            Some(r) => say(Msg::TrayOk, &[("time", card_time(r, cx))]),
            None => say(Msg::TrayNoReset, &[]),
        },
        RowState::OkNoReset | RowState::Idle | RowState::OkBalance => say(Msg::TrayNoReset, &[]),
        RowState::Warning => match t_out(a) {
            Some(t) if is_balance => {
                say(Msg::TrayBalanceWarning, &[("time", runout_point(a, t, cx))])
            }
            Some(t) => say(Msg::TrayWarning, &[("time", runout_point(a, t, cx))]),
            None => say(Msg::TrayLevel, &[]),
        },
        RowState::CriticalRunway => match t_out(a) {
            Some(t) => {
                let n = runway_minutes(cx.now, t);
                say(
                    if is_balance {
                        Msg::TrayBalanceRunway
                    } else {
                        Msg::TrayRunway
                    },
                    &[("duration", countdown(n, CountdownForm::Compact, cx.strings))],
                )
            }
            None => say(Msg::TrayLevel, &[]),
        },
        RowState::CriticalLevel => say(Msg::TrayLevel, &[]),
        RowState::Exhausted => match reset {
            Some(r) => say(Msg::TrayExhausted, &[("time", card_time(r, cx))]),
            None if is_balance => say(Msg::TrayBalanceEmpty, &[]),
            None => say(Msg::TrayLevel, &[]),
        },
    };
    truncate_chars(&tip, TRAY_TIP_MAX_CHARS)
}

/// `s` cut to at most `max` characters, the last one an ellipsis when cut (`szTip` holds 127).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_names() {
        use crate::grammar::strings::{English, Plural, PluralMsg};

        let en = &English;
        assert_eq!(spoken_name("5-hour session", en), "5-hour session");
        assert_eq!(spoken_name("Weekly · all models", en), "weekly, all models");
        assert_eq!(spoken_name("Weekly · Fable", en), "weekly, Fable");
        assert_eq!(spoken_name("Credits", en), "credits");
        assert_eq!(spoken_name("", en), "");

        // A language that capitalises nouns keeps the capital (German `Woche, alle Modelle`).
        struct Capitals;
        impl Strings for Capitals {
            fn language(&self) -> &str {
                "de"
            }
            fn template(&self, msg: Msg) -> &str {
                English.template(msg)
            }
            fn plural(&self, n: i64) -> Plural {
                English.plural(n)
            }
            fn plural_template(&self, msg: PluralMsg, form: Plural) -> &str {
                English.plural_template(msg, form)
            }
            fn lowercase_names(&self) -> bool {
                false
            }
        }
        assert_eq!(
            spoken_name("Woche · alle Modelle", &Capitals),
            "Woche, alle Modelle"
        );
    }

    #[test]
    fn tooltips_are_cut_to_127_characters() {
        let long = "x".repeat(200);
        let cut = truncate_chars(&long, 127);
        assert_eq!(cut.chars().count(), 127);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_chars("short", 127), "short");
        assert_eq!(first_sentence("A b. C d."), "A b.");
    }
}
