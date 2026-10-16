//! Meter names in the language of the strings (R§2 table, R§5.9).
//!
//! Providers name their meters in English: the catalogue ([`crate::model::catalog`]), Claude's
//! per-model weekly windows ([`MeterNames::weekly_model`]), Codex's windows named by their length
//! ([`length_names`]) and Copilot's quotas. Readings, the history and the state file keep those
//! names; the surfaces show [`localized`] ones, which the app puts into every evaluation with
//! [`localize`]. A meter this build has no names for keeps the ones its provider gave.

use super::strings::{Msg, Strings, text};
use crate::model::{MeterId, MeterNames, ProviderId};
use crate::stats::Evaluation;

/// The `short_label`, `short_name` and `name` messages of a meter.
type NameMsgs = (Msg, Msg, Msg);

const FIVE_HOUR: NameMsgs = (
    Msg::MeterFiveHourLabel,
    Msg::MeterFiveHourShort,
    Msg::MeterFiveHourName,
);
const WEEKLY: NameMsgs = (
    Msg::MeterWeeklyLabel,
    Msg::MeterWeeklyShort,
    Msg::MeterWeeklyName,
);
const WEEKLY_ALL: NameMsgs = (
    Msg::MeterWeeklyLabel,
    Msg::MeterWeeklyShort,
    Msg::MeterWeeklyAllName,
);
const HOURS: NameMsgs = (
    Msg::MeterHoursLabel,
    Msg::MeterHoursShort,
    Msg::MeterHoursName,
);
const DAYS: NameMsgs = (Msg::MeterDaysLabel, Msg::MeterDaysShort, Msg::MeterDaysName);
const CREDITS: NameMsgs = (
    Msg::MeterCreditsLabel,
    Msg::MeterCreditsShort,
    Msg::MeterCreditsName,
);
const PREMIUM: NameMsgs = (
    Msg::MeterPremiumLabel,
    Msg::MeterPremiumShort,
    Msg::MeterPremiumName,
);
const CHAT: NameMsgs = (Msg::MeterChatLabel, Msg::MeterChatShort, Msg::MeterChatName);
const COMPLETIONS: NameMsgs = (
    Msg::MeterCompletionsLabel,
    Msg::MeterCompletionsShort,
    Msg::MeterCompletionsName,
);

/// The three names from their messages, each filled with `args`.
fn names_of(strings: &dyn Strings, msgs: NameMsgs, args: &[(&str, &str)]) -> MeterNames {
    let (label, short, name) = msgs;
    MeterNames {
        short_label: text(strings, label, args),
        short_name: text(strings, short, args),
        name: text(strings, name, args),
    }
}

/// The names of a window outside the catalogue, by its length: whole hours below two days
/// (`24h` / `24-hour` / `24-hour window`), else whole days (`30d` / `30-day` / `30-day window`);
/// at least 1.
pub fn length_names(minutes: i64, strings: &dyn Strings) -> MeterNames {
    const DAY: i64 = 24 * 60;
    let (n, msgs) = if minutes < 2 * DAY {
        ((minutes + 30) / 60, HOURS)
    } else {
        ((minutes + DAY / 2) / DAY, DAYS)
    };
    let n = n.max(1).to_string();
    names_of(strings, msgs, &[("n", &n)])
}

/// The names of the meter `id` of `provider` in the language of `strings`, given the names its
/// provider gave (`names`): the catalogue meters, Copilot's quotas and Codex's windows by their
/// id; a per-model weekly window with the model of `names` and its `short_label` (which the
/// provider may have shortened); a Copilot quota this build does not know with the `name` of
/// `names`. Any other meter keeps `names`. With [`super::English`] the result is the provider's
/// names.
pub fn localized(
    provider: ProviderId,
    id: &MeterId,
    names: &MeterNames,
    strings: &dyn Strings,
) -> MeterNames {
    let fixed = |msgs: NameMsgs| names_of(strings, msgs, &[]);
    match (provider, id.0.as_str()) {
        (ProviderId::Claude | ProviderId::Codex, MeterId::FIVE_HOUR) => fixed(FIVE_HOUR),
        (ProviderId::Claude, MeterId::WEEKLY) => fixed(WEEKLY_ALL),
        (ProviderId::Codex, MeterId::WEEKLY) => fixed(WEEKLY),
        (ProviderId::Meshy, MeterId::CREDITS) => fixed(CREDITS),
        (ProviderId::Copilot, MeterId::PREMIUM) => fixed(PREMIUM),
        (ProviderId::Copilot, MeterId::CHAT) => fixed(CHAT),
        (ProviderId::Copilot, MeterId::COMPLETIONS) => fixed(COMPLETIONS),
        (ProviderId::Copilot, _) => MeterNames {
            short_label: strings.template(Msg::MeterMonthlyLabel).to_owned(),
            short_name: strings.template(Msg::MeterMonthlyShort).to_owned(),
            name: names.name.clone(),
        },
        (ProviderId::Claude, _) if id.is_weekly_model() => match names.weekly_model_name() {
            Some(model) => MeterNames {
                short_label: names.short_label.clone(),
                short_name: text(strings, Msg::MeterWeeklyModelShort, &[("model", model)]),
                name: text(strings, Msg::MeterWeeklyModelName, &[("model", model)]),
            },
            None => names.clone(),
        },
        (ProviderId::Codex, _) => match id.window_minutes() {
            Some(minutes) => length_names(minutes, strings),
            None => names.clone(),
        },
        _ => names.clone(),
    }
}

/// Puts [`localized`] names into every meter of `eval`: its assessments and the meters its
/// services reported earlier ([`crate::stats::ServiceEval::absent`]), so every surface built
/// from it, and the readout's widths (R§6.3), use the names of `strings`.
pub fn localize(eval: &mut Evaluation, strings: &dyn Strings) {
    for service in &mut eval.services {
        for a in &mut service.meters {
            a.names = localized(a.key.provider, &a.key.meter, &a.names, strings);
        }
        for m in &mut service.absent {
            m.names = localized(m.key.provider, &m.key.meter, &m.names, strings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::{English, Swedish};
    use crate::model::catalog;

    fn names(short_label: &str, short_name: &str, name: &str) -> MeterNames {
        MeterNames::new(short_label, short_name, name)
    }

    #[test]
    fn english_names_are_the_providers_names() {
        for provider in ProviderId::ALL {
            for def in catalog(provider) {
                assert_eq!(
                    localized(provider, &def.meter_id(), &def.names(), &English),
                    def.names(),
                    "{provider:?} {}",
                    def.id
                );
            }
        }
        let fable = MeterNames::weekly_model("Fable");
        assert_eq!(
            localized(
                ProviderId::Claude,
                &MeterId::weekly_model("fable"),
                &fable,
                &English
            ),
            fable
        );
        assert_eq!(
            length_names(24 * 60, &English),
            names("24h", "24-hour", "24-hour window")
        );
        assert_eq!(
            length_names(30 * 24 * 60, &English),
            names("30d", "30-day", "30-day window")
        );
        assert_eq!(length_names(10, &English).short_label, "1h");
    }

    #[test]
    fn swedish_names_say_limit_in_the_short_name() {
        let sv = |provider: ProviderId, id: &str| {
            localized(provider, &MeterId::new(id), &names("x", "x", "x"), &Swedish)
        };
        assert_eq!(
            sv(ProviderId::Claude, MeterId::FIVE_HOUR),
            names("5h", "5-timmarsgränsen", "5-timmarssession")
        );
        assert_eq!(
            sv(ProviderId::Claude, MeterId::WEEKLY),
            names("Vk", "veckogränsen", "Vecka · alla modeller")
        );
        assert_eq!(sv(ProviderId::Codex, MeterId::WEEKLY).name, "Vecka");
        assert_eq!(
            sv(ProviderId::Meshy, MeterId::CREDITS),
            names("Kred", "krediterna", "Krediter")
        );
        assert_eq!(
            sv(ProviderId::Copilot, MeterId::PREMIUM).short_name,
            "premiumgränsen"
        );
        // A quota this build does not know keeps the id as its name.
        assert_eq!(
            localized(
                ProviderId::Copilot,
                &MeterId::new("agents"),
                &names("Mo", "monthly", "agents"),
                &Swedish
            ),
            names("Mnd", "månadsgränsen", "agents")
        );
        assert_eq!(
            localized(
                ProviderId::Codex,
                &MeterId::window(24 * 60),
                &length_names(24 * 60, &English),
                &Swedish
            ),
            names("24h", "24-timmarsgränsen", "24-timmarsfönster")
        );
    }

    #[test]
    fn a_per_model_weekly_window_keeps_its_model_and_label() {
        let mut opus = MeterNames::weekly_model("Opus 4.5");
        opus.short_label = "Opus…".to_owned();
        assert_eq!(
            localized(
                ProviderId::Claude,
                &MeterId::weekly_model("opus-4-5"),
                &opus,
                &Swedish
            ),
            names("Opus…", "Opus 4.5-veckogränsen", "Vecka · Opus 4.5")
        );
    }

    #[test]
    fn unknown_meters_keep_their_names() {
        let odd = names("Odd", "odd", "Odd window");
        for (provider, id) in [
            (ProviderId::Claude, MeterId::new("odd")),
            (ProviderId::Codex, MeterId::new("odd")),
            (ProviderId::Meshy, MeterId::new("odd")),
            // Names that did not come from `MeterNames::weekly_model`.
            (ProviderId::Claude, MeterId::weekly_model("odd")),
        ] {
            assert_eq!(localized(provider, &id, &odd, &Swedish), odd, "{id:?}");
        }
    }
}
