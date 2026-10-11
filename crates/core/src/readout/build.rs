//! Builds the readout model from an evaluation (R§4.3, R§5.7, R§6.3, R§6.4, R§11).

use super::bar::bar_input;
use super::layout::{BalanceWidthInputs, WidthInputs};
use super::{ContentForm, ReadoutModel, ReadoutUia, RowModel, TextStyle};
use crate::color::ColorRole;
use crate::grammar::TextContext;
use crate::grammar::a11y::{root_name, row_sentence, service_sentence};
use crate::grammar::cells::{RowPosition, compact_cell, frozen_exhausted, pct, right_cell, run};
use crate::grammar::numbers::percent;
use crate::grammar::strings::{Msg, Strings, text};
use crate::grammar::{LocaleFormat, TimeStyle};
use crate::model::{MeterKind, MeterNames, catalog};
use crate::settings::{ReadoutContent, Settings};
use crate::stats::{AssessedMeter, Assessment, Evaluation, LiveChange, RowState, ServiceEval};

/// What every surface builder reads besides the evaluation.
#[derive(Clone, Copy)]
pub struct BuildContext<'a> {
    /// Clock, time style, locale and strings.
    pub text: TextContext<'a>,
    /// Settings: content mode and pins (R§4.3), enabled services and hidden meters (which meters
    /// *can* appear, for widths and the glyph column, R§6.3–R§6.4).
    pub settings: &'a Settings,
}

/// The widest numbers the width strings are measured with (digits are tabular, R§5.6, so any
/// two-digit number stands for every two-digit number).
const WIDE_DAYS: i64 = 88;
const WIDE_MINUTES: i64 = 59;
const WIDE_HOURS: i64 = 47;
const WIDE_COUNTDOWN_DAYS: i64 = 29;
const WIDE_RELATIVE_HOURS: i64 = 23;
const WIDE_BALANCE_DAYS: i64 = 13;
const WIDE_BALANCE_WEEKS: i64 = 8;
const WIDE_PCT: u8 = 99;

/// The label of a meter in the readout: `short_label`, with the account tag when the service has
/// several accounts (R§6.4: `W 5h`).
pub(crate) fn row_label(eval: &Evaluation, service: &ServiceEval, names: &MeterNames) -> String {
    let provider = service.source.provider;
    let accounts: Vec<&ServiceEval> = eval
        .services
        .iter()
        .filter(|s| s.source.provider == provider)
        .collect();
    if accounts.len() < 2 {
        return names.short_label.clone();
    }
    // Default tag: the first letter of the alias, else 1, 2, … in the order of the accounts.
    let tag = service.account.tag.clone().unwrap_or_else(|| {
        service
            .account
            .alias
            .as_ref()
            .and_then(|a| a.chars().next())
            .map(String::from)
            .unwrap_or_else(|| {
                let index = accounts
                    .iter()
                    .position(|s| s.source == service.source)
                    .unwrap_or(0);
                (index + 1).to_string()
            })
    });
    format!("{tag} {}", names.short_label)
}

/// `{n}` filled into a message.
fn num(strings: &dyn Strings, msg: Msg, n: i64) -> String {
    text(strings, msg, &[("n", &n.to_string())])
}

/// The widest short time of the locale: two hour digits, as tabular digits make every hour
/// as wide (`00:00`; 12-hour clocks never show `00`, so `12:00`).
fn time_sample(locale: &LocaleFormat) -> String {
    let hour = if locale.clock.hour24 { "00" } else { "12" };
    format!("{hour}{}00", locale.clock.separator)
}

/// Every TIME token the windows can produce (R§6.3 `TIMEW`).
fn time_tokens(cx: &TextContext<'_>) -> Vec<String> {
    let s = cx.strings;
    match cx.time_style {
        TimeStyle::Clock => {
            let mut tokens = vec![time_sample(cx.locale), s.template(Msg::Today).to_owned()];
            tokens.extend(cx.locale.weekdays_abbrev.iter().cloned());
            tokens.push(num(s, Msg::TokenDays, WIDE_DAYS));
            tokens
        }
        TimeStyle::Relative => vec![
            num(s, Msg::TokenRelMin, WIDE_MINUTES),
            num(s, Msg::TokenRelHours, WIDE_RELATIVE_HOURS),
            num(s, Msg::TokenRelDays, WIDE_DAYS),
        ],
    }
}

/// One meter that can appear in the readout: its service and names, and whether it is a
/// balance.
struct CanAppear<'a> {
    service: &'a ServiceEval,
    names: MeterNames,
    balance: bool,
}

/// What can appear in the readout (R§6.3, R§6.4): the pins in pinned mode; else every
/// non-hidden catalogue meter of every enabled service plus every meter the evaluation holds.
fn can_appear<'a>(eval: &'a Evaluation, settings: &Settings) -> Vec<CanAppear<'a>> {
    let is_balance = |a: &Assessment| match &a.meter {
        AssessedMeter::Balance(_) => true,
        AssessedMeter::Window(_) => false,
        AssessedMeter::Loading => catalog(a.key.provider)
            .iter()
            .any(|d| d.id == a.key.meter.0 && d.kind == MeterKind::Balance),
    };
    if let ReadoutContent::Pinned(pins) = &settings.readout.content {
        // A pin whose meter the latest reading lacks keeps its room (`ServiceEval::absent`).
        let pinned: Vec<CanAppear<'a>> = pins
            .iter()
            .filter_map(|k| match eval.assessment(k) {
                Some((service, a)) => Some(CanAppear {
                    service,
                    names: a.names.clone(),
                    balance: is_balance(a),
                }),
                None => eval.services.iter().find_map(|service| {
                    let m = service.absent.iter().find(|m| &m.key == k)?;
                    Some(CanAppear {
                        service,
                        names: m.names.clone(),
                        balance: m.balance,
                    })
                }),
            })
            .collect();
        if !pinned.is_empty() {
            return pinned;
        }
    }
    let mut out = Vec::new();
    for service in &eval.services {
        for a in &service.meters {
            out.push(CanAppear {
                service,
                names: a.names.clone(),
                balance: is_balance(a),
            });
        }
        // Meters reported earlier (in this run, or in an earlier one the state file remembers)
        // that the latest reading lacks: widths never change with data (R§6.3).
        for m in &service.absent {
            out.push(CanAppear {
                service,
                names: m.names.clone(),
                balance: m.balance,
            });
        }
        let hidden = settings
            .provider(service.source.provider)
            .map_or(&[][..], |p| p.hidden_meters.as_slice());
        for def in catalog(service.source.provider) {
            let reported = service.meters.iter().any(|a| a.key.meter.0 == def.id);
            if !reported && !hidden.iter().any(|h| h.0 == def.id) {
                out.push(CanAppear {
                    service,
                    names: def.names(),
                    balance: def.kind == MeterKind::Balance,
                });
            }
        }
    }
    out
}

/// The strings that define the column widths (R§6.3), from what can appear, the locale, the
/// strings and the time style; never from values.
fn width_inputs(eval: &Evaluation, cx: &BuildContext<'_>) -> WidthInputs {
    let t = &cx.text;
    let s = t.strings;
    let appear = can_appear(eval, cx.settings);
    let mut labels: Vec<String> = appear
        .iter()
        .map(|m| row_label(eval, m.service, &m.names))
        .collect();
    labels.sort();
    labels.dedup();

    let tokens = time_tokens(t);
    let prefixed = |msg: Msg| -> Vec<String> {
        tokens
            .iter()
            .map(|tok| text(s, msg, &[("time", tok)]))
            .collect()
    };
    let mut statuses: Vec<String> = [
        Msg::ReasonOffline,
        Msg::ReasonSignIn,
        Msg::ReasonPaused,
        Msg::ReasonNoData,
        Msg::NoReset,
    ]
    .iter()
    .map(|m| s.template(*m).to_owned())
    .collect();
    statuses.push(num(s, Msg::AgeMin, WIDE_MINUTES));
    statuses.push(num(s, Msg::AgeHours, WIDE_HOURS));
    statuses.push(num(s, Msg::AgeDays, WIDE_DAYS));

    let mut regular_at_zero = vec![s.template(Msg::Loading).to_owned()];
    regular_at_zero.extend(prefixed(Msg::Back));

    let mut say_semibold = prefixed(Msg::Out);
    say_semibold.push(num(s, Msg::CountdownMin, WIDE_MINUTES));
    say_semibold.push(num(s, Msg::CountdownHours, WIDE_HOURS));
    say_semibold.push(num(s, Msg::CountdownDays, WIDE_COUNTDOWN_DAYS));
    say_semibold.extend(prefixed(Msg::Back));
    say_semibold.push(s.template(Msg::Empty).to_owned());

    let balance = appear
        .iter()
        .any(|m| m.balance)
        .then(|| BalanceWidthInputs {
            approx: s.template(Msg::BalanceApprox).to_owned(),
            durations: vec![
                num(s, Msg::BalanceHours, WIDE_HOURS),
                num(s, Msg::BalanceDays, WIDE_BALANCE_DAYS),
                num(s, Msg::BalanceWeeks, WIDE_BALANCE_WEEKS),
                s.template(Msg::BalanceMax).to_owned(),
            ],
        });

    let pct99 = percent(WIDE_PCT, t.locale);
    WidthInputs {
        glyph_column: eval.multi_service,
        labels,
        time_tokens: tokens,
        statuses,
        regular_at_zero,
        say_semibold,
        balance,
        compact_semibold: vec![
            num(s, Msg::CountdownCompactMin, WIDE_MINUTES),
            num(s, Msg::CountdownCompactHours, WIDE_HOURS),
            num(s, Msg::CountdownCompactDays, WIDE_COUNTDOWN_DAYS),
            s.template(Msg::Empty).to_owned(),
            pct99.clone(),
        ],
        compact_regular: vec![pct99],
    }
}

/// The sentences of the rows for the root's name (R§11): a stale service is one sentence for
/// all of its rows (`Claude: offline, … Then: 5-hour session 42 percent used, …`).
fn row_sentences(rows: &[(&ServiceEval, &Assessment)], cx: &TextContext<'_>) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut described = Vec::new();
    for (service, a) in rows {
        if matches!(a.state, RowState::Stale(_)) {
            if described.contains(&&service.source) {
                continue;
            }
            described.push(&service.source);
            let same: Vec<&Assessment> = rows
                .iter()
                .filter(|(s, _)| s.source == service.source)
                .map(|(_, a)| *a)
                .collect();
            sentences.push(service_sentence(service, &same, cx));
        } else {
            sentences.push(row_sentence(a, service, cx));
        }
    }
    sentences
}

/// The readout model (R§4–R§6, R§11): rows for `eval.readout_rows` in display order, each with
/// label, bar input (calm fill TextPrimary), right cell (R§5.7) and compact cell (R§5.8); the
/// compact row (the winner among the rows); the width inputs from what *can* appear (R§6.3);
/// UIA names; `ContentForm::Hidden` when the readout is off or there is nothing to show
/// (R§3.10 "No meter at all").
///
/// "What can appear" (R§6.3, R§6.4) is: pinned mode — the pins; auto mode — every non-hidden
/// meter of every enabled service, i.e. the catalogue meters ([`crate::model::catalog`]) plus
/// the meters the evaluation holds that are not in the catalogue (Claude's per-model weeklies,
/// so `Fable` widens the label column once it has been reported) plus the meters a source
/// reported earlier (this run or a remembered one) and its latest reading lacks
/// ([`crate::stats::ServiceEval::absent`]). Balance strings are included only when a balance
/// meter can appear. The width inputs therefore change with settings, the locale or a newly
/// reported meter, never with values or a reading that lacks a meter.
pub fn build_readout(eval: &Evaluation, cx: &BuildContext<'_>) -> ReadoutModel {
    let t = &cx.text;
    let shown: Vec<(&ServiceEval, &Assessment)> = eval
        .readout_rows
        .iter()
        .filter_map(|k| eval.assessment(k))
        .collect();
    let content = if !cx.settings.readout.enabled || shown.is_empty() {
        ContentForm::Hidden
    } else if shown.len() == 1 {
        ContentForm::SingleRow
    } else {
        ContentForm::TwoRows
    };

    let rows: Vec<RowModel> = shown
        .iter()
        .enumerate()
        .map(|(i, (service, a))| {
            let siblings: Vec<usize> = shown
                .iter()
                .enumerate()
                .filter(|(_, (s, _))| s.source == service.source)
                .map(|(j, _)| j)
                .collect();
            let mut pos = RowPosition {
                index_in_service: siblings.iter().position(|&j| j == i).unwrap_or(0),
                rows_in_service: siblings.len(),
            };
            // R§5.7: a frozen used-up row has no status; its sibling takes the one-row status.
            let sibling_frozen = siblings
                .iter()
                .any(|&j| j != i && frozen_exhausted(shown[j].1));
            if sibling_frozen && !frozen_exhausted(a) {
                pos = RowPosition {
                    index_in_service: 0,
                    rows_in_service: 1,
                };
            }
            RowModel {
                key: a.key.clone(),
                glyph: eval.multi_service.then(|| service.source.provider.glyph()),
                label: run(
                    row_label(eval, service, &a.names),
                    TextStyle::LABEL,
                    ColorRole::TextSecondary,
                ),
                bar: bar_input(a, ColorRole::TextPrimary),
                right: right_cell(a, service, pos, t),
                compact: compact_cell(a, t),
                uia_name: row_sentence(a, service, t),
                uia_value: (!matches!(a.state, RowState::Loading | RowState::Ended))
                    .then(|| pct(a)),
            }
        })
        .collect();

    // The compact form shows the winner among the rows: the row ranked highest.
    let compact_row = rows
        .iter()
        .enumerate()
        .min_by_key(|(_, r)| {
            eval.ranked
                .iter()
                .position(|k| k == &r.key)
                .unwrap_or(usize::MAX)
        })
        .map_or(0, |(i, _)| i);

    let sentences = row_sentences(&shown, t);
    let compact_sentence = shown
        .get(compact_row)
        .map(|(service, a)| row_sentence(a, service, t));
    let announce = eval.changes.iter().any(|c| match c {
        LiveChange::SeverityRose(k) | LiveChange::WindowReset(k) => eval.readout_rows.contains(k),
        LiveChange::FreshnessChanged(source) => {
            eval.readout_rows.iter().any(|k| &k.source() == source)
        }
    });

    ReadoutModel {
        content,
        glyph_column: eval.multi_service,
        rows,
        compact_row,
        widths: width_inputs(eval, cx),
        uia: ReadoutUia {
            root_name: root_name(&sentences, t),
            root_name_compact: root_name(compact_sentence.as_slice(), t),
            help_text: t.strings.template(Msg::A11yHelp).to_owned(),
            announce,
        },
    }
}
