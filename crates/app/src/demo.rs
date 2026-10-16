//! Built-in demo data for `--preview-flyout --demo`: the reference content of the design mock
//! (docs/design/final/flyout.html §1, F§1.2 "Claude 3 meters, Codex 1 meter, Meshy balance"),
//! as invented snapshots and history run through `lastdrop-core`'s real pipeline.
//!
//! Everything here is invented (no account, no token, no file of this machine), and nothing
//! touches the network, the registry or Windows APIs, so the preview can run anywhere.
//!
//! The clock is R§13's: Sat 2026-10-10 12:42 in a fixed UTC+02:00 zone, en-GB format and
//! English; [`Demo::in_language`] shows it in another language with that language's format
//! (Swedish: sv-SE).

use std::collections::BTreeMap;

use jiff::tz::{self, TimeZone};
use jiff::{SignedDuration, Timestamp, civil};
use lastdrop_core::flyout::{FlyoutModel, build_flyout};
use lastdrop_core::grammar::meters::localize;
use lastdrop_core::grammar::{Language, LocaleFormat, TextContext, TimeStyle};
use lastdrop_core::model::{
    AccountKey, AccountRef, Balance, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId,
    Snapshot, SourceKind, Unit, WindowLen, catalog,
};
use lastdrop_core::readout::BuildContext;
use lastdrop_core::readout::hovercard::{HoverAnchor, HoverCardModel, build_hover_card};
use lastdrop_core::settings::{ProviderEntry, ProviderSettings, Settings};
use lastdrop_core::stats::{
    EvalInput, EvalMemory, Evaluation, FailureKind, Sample, ServiceInput, ServiceStatus, evaluate,
};
use lastdrop_win::palette::Argb;
use lastdrop_win::theme::{AccentShades, ThemeInfo};

/// Use per day in percentage points of the Claude weekly window, oldest first (the mock's
/// sparkline).
const CLAUDE_WEEKLY_DAYS: [f64; 14] = [6., 9., 4., 0., 0., 11., 8., 7., 12., 5., 0., 3., 9., 6.];
/// The same for Codex weekly.
const CODEX_WEEKLY_DAYS: [f64; 14] = [1., 0., 2., 0., 0., 1., 3., 0., 1., 0., 0., 2., 1., 1.];
/// Meshy credits spent per day; the last 7 days add up to 952 (136 a day, R§13 "Balance").
const MESHY_DAYS: [f64; 14] = [
    120., 0., 250., 90., 140., 0., 0., 180., 150., 95., 200., 127., 60., 140.,
];

/// Which state of the mock the demo shows (flyout.html §1–§3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scenario {
    /// §1: three services, all on track (12:42).
    #[default]
    Normal,
    /// §2: 13:45, Claude's 5-hour session runs out before its reset (`Out 15:18`).
    Warning,
    /// §3: 13:29, Claude has not answered for 47 minutes (HTTP 503).
    Stale,
    /// Codex's sign-in expired (F§4.3 SIGN_IN message line).
    SignIn,
}

impl Scenario {
    /// Parses `normal`, `warning`, `stale` or `signin`.
    pub fn parse(name: &str) -> Option<Scenario> {
        match name.to_ascii_lowercase().as_str() {
            "normal" | "ok" => Some(Scenario::Normal),
            "warning" | "warn" => Some(Scenario::Warning),
            "stale" => Some(Scenario::Stale),
            "signin" | "sign-in" => Some(Scenario::SignIn),
            _ => None,
        }
    }
}

/// One demo service: what `stats::evaluate` reads for it.
#[derive(Clone, Debug)]
pub struct DemoService {
    /// The account as configured.
    pub account: AccountRef,
    /// Its latest reading.
    pub snapshot: Snapshot,
    /// Its scheduler status.
    pub status: ServiceStatus,
}

/// The demo world: clock, settings, format, theme, services and history.
#[derive(Clone, Debug)]
pub struct Demo {
    /// `now` (Sat 2026-10-10 12:42 local).
    pub now: Timestamp,
    /// Claude, Codex and Meshy enabled, in that order.
    pub settings: Settings,
    /// en-GB in UTC+02:00 (sv-SE in Swedish).
    pub locale: LocaleFormat,
    /// The language of the strings (English unless [`Demo::in_language`]).
    pub language: Language,
    /// Light or dark Windows mode with the owner's accent (R§7: Dark1 `#8E3AA7`, Light2
    /// `#D58AE8`), transparency off.
    pub theme: ThemeInfo,
    /// The services, in service order.
    pub services: Vec<DemoService>,
    /// 14+ days of history (sparklines, forecasts, the Meshy burn rate).
    pub history: BTreeMap<MeterKey, Vec<Sample>>,
}

/// The demo zone: UTC+02:00.
fn zone() -> TimeZone {
    TimeZone::fixed(tz::offset(2))
}

/// A local time in October 2026 (September for days ≤ 0, counting back from October 1).
fn local(day: i8, hour: i8, minute: i8) -> Timestamp {
    let (month, day) = if day <= 0 { (9, 30 + day) } else { (10, day) };
    civil::date(2026, month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(zone())
        .map(|z| z.timestamp())
        .unwrap_or(Timestamp::UNIX_EPOCH)
}

fn key(provider: ProviderId, meter: &MeterId) -> MeterKey {
    MeterKey {
        provider,
        account: AccountKey::primary(),
        meter: meter.clone(),
    }
}

/// A window meter from its catalogue definition.
fn window_meter(provider: ProviderId, index: usize, used: f64, resets_at: Timestamp) -> Meter {
    let defs = catalog(provider);
    let def = defs[index.min(defs.len() - 1)];
    let (window, starts_on_use) = match def.kind {
        lastdrop_core::model::MeterKind::Window {
            window,
            starts_on_use,
        } => (window, starts_on_use),
        lastdrop_core::model::MeterKind::Balance => (WindowLen::Unknown, false),
    };
    Meter {
        id: def.meter_id(),
        names: def.names(),
        order: def.order,
        used,
        limited: false,
        resets_at: Some(resets_at),
        window,
        starts_on_use,
    }
}

fn snapshot(provider: ProviderId, plan: &str, at: Timestamp) -> Snapshot {
    Snapshot {
        provider,
        account: AccountRef {
            plan: Some(plan.to_owned()),
            ..AccountRef::primary()
        },
        source: match provider {
            ProviderId::Claude => SourceKind::ClaudeOAuthUsage,
            ProviderId::Codex => SourceKind::CodexAppServer,
            ProviderId::Copilot => SourceKind::CopilotApi,
            ProviderId::Meshy => SourceKind::MeshyApi,
        },
        measured_at: at,
        fetched_at: at,
        meters: Vec::new(),
        balances: Vec::new(),
        health: Health::Ok,
    }
}

fn fresh(at: Timestamp) -> ServiceStatus {
    ServiceStatus {
        last_success: Some(at),
        poll_interval: Some(SignedDuration::from_mins(5)),
        ..ServiceStatus::default()
    }
}

/// Two readings a day of a weekly window (10:00 and 12:30 local; today ends with the latest
/// reading at `last`), rising by `days[k]` points on day k. A reading after a reset starts a new
/// window instance at `starts[instance]`, the first reading after that reset.
fn weekly_history(
    days: &[f64; 14],
    resets: &[Timestamp],
    starts: &[f64],
    last: Timestamp,
) -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut used = None::<(Timestamp, f64)>;
    for (k, v) in days.iter().enumerate() {
        // 14 days ending today, Sat 10 Oct: day 0 is Sun 27 Sep (`local(-3, …)`).
        let day = i8::try_from(k).unwrap_or(0) - 3;
        let today = k == days.len() - 1;
        let morning = local(day, 10, 0);
        let evening = if today { last } else { local(day, 12, 30) };
        let instance = resets.iter().position(|r| *r > morning).unwrap_or(0);
        let reset = resets.get(instance).copied();
        let start = match used {
            Some((r, u)) if Some(r) == reset => u,
            _ => starts.get(instance).copied().unwrap_or(0.0),
        };
        let end = start + v / 100.0;
        samples.push(Sample::window(morning, start, reset));
        samples.push(Sample::window(evening, end, reset));
        if let Some(r) = reset {
            used = Some((r, end));
        }
    }
    samples
}

/// `t` plus `minutes`.
fn plus(t: Timestamp, minutes: i64) -> Timestamp {
    t.checked_add(SignedDuration::from_mins(minutes))
        .unwrap_or(t)
}

impl Demo {
    /// The demo world (scenario [`Scenario::Normal`]) in the light (`dark = false`) or dark
    /// Windows mode.
    pub fn new(dark: bool) -> Demo {
        Demo::scenario(dark, Scenario::Normal)
    }

    /// The demo world of `scenario`.
    pub fn scenario(dark: bool, scenario: Scenario) -> Demo {
        let later = match scenario {
            Scenario::Normal | Scenario::SignIn => 0,
            Scenario::Warning => 63,
            Scenario::Stale => 47,
        };
        let now = plus(local(10, 12, 42), later);
        let claude_at = match scenario {
            // The last good reading of a service that stopped answering at 12:42.
            Scenario::Stale => local(10, 12, 42),
            _ => plus(now, -1),
        };
        let codex_at = plus(now, -2);
        let meshy_at = plus(now, -1);

        let five_reset = local(10, 14, 56);
        let week_reset = local(12, 9, 0);
        let mut claude = snapshot(ProviderId::Claude, "Max", claude_at);
        let fable = Meter {
            id: MeterId::weekly_model("fable"),
            names: MeterNames::weekly_model("Fable"),
            order: 2,
            ..window_meter(ProviderId::Claude, 1, 0.18, week_reset)
        };
        let (five_used, five_reset) = match scenario {
            Scenario::Warning => (0.78, local(10, 16, 30)),
            _ => (0.42, five_reset),
        };
        claude.meters = vec![
            window_meter(ProviderId::Claude, 0, five_used, five_reset),
            window_meter(ProviderId::Claude, 1, 0.61, week_reset),
            fable,
        ];

        let codex_reset = local(16, 14, 26);
        let mut codex = snapshot(ProviderId::Codex, "Pro Lite", codex_at);
        codex.meters = vec![window_meter(ProviderId::Codex, 1, 0.05, codex_reset)];

        let mut meshy = snapshot(ProviderId::Meshy, "Pro", meshy_at);
        let credits = catalog(ProviderId::Meshy)[0];
        meshy.balances = vec![Balance {
            id: credits.meter_id(),
            names: credits.names(),
            order: credits.order,
            amount: 1240.0,
            unit: Unit::Credits,
            last_top_up: None,
        }];

        let mut history = BTreeMap::new();
        let five = MeterId::new(MeterId::FIVE_HOUR);
        let weekly = MeterId::new(MeterId::WEEKLY);
        // R§13's OK forecast vector: (12:12, .375), (12:42, .42), here one minute earlier; the
        // warning: (13:15, .71), (13:45, .78), also one minute earlier.
        let five_before = match scenario {
            Scenario::Warning => 0.71,
            _ => 0.375,
        };
        history.insert(
            key(ProviderId::Claude, &five),
            vec![
                Sample::window(plus(claude_at, -30), five_before, Some(five_reset)),
                Sample::window(claude_at, five_used, Some(five_reset)),
            ],
        );
        history.insert(
            key(ProviderId::Claude, &weekly),
            weekly_history(
                &CLAUDE_WEEKLY_DAYS,
                &[local(-2, 9, 0), local(5, 9, 0), week_reset],
                &[0.80, 0.30, 0.26],
                claude_at,
            ),
        );
        history.insert(
            key(ProviderId::Claude, &MeterId::weekly_model("fable")),
            vec![Sample::window(claude_at, 0.18, Some(week_reset))],
        );
        history.insert(
            key(ProviderId::Codex, &weekly),
            weekly_history(
                &CODEX_WEEKLY_DAYS,
                &[
                    local(-5, 14, 26),
                    local(2, 14, 26),
                    local(9, 14, 26),
                    codex_reset,
                ],
                &[0.40, 0.20, 0.07, 0.04],
                codex_at,
            ),
        );
        // Meshy: a top-up to 3,260 on 25 Sep (R§13 `ref`), 468 spent on 26 Sep, then the days
        // above at 10:00; the latest reading at 12:41.
        let mut credits_history = vec![
            Sample::balance(local(-5, 8, 0), 400.0),
            Sample::balance(local(-5, 8, 5), 3260.0),
            Sample::balance(local(-4, 10, 0), 2792.0),
        ];
        let mut amount = 2792.0;
        for (k, spent) in MESHY_DAYS.iter().enumerate() {
            let day = i8::try_from(k).unwrap_or(0) - 3;
            credits_history.push(Sample::balance(local(day, 9, 0), amount));
            amount -= spent;
            credits_history.push(Sample::balance(local(day, 10, 0), amount));
        }
        credits_history.push(Sample::balance(meshy_at, amount));
        history.insert(key(ProviderId::Meshy, &credits.meter_id()), credits_history);

        // The demo has readings of Claude, Codex and Meshy; Copilot is listed but off.
        let settings = Settings {
            providers: ProviderId::ALL
                .into_iter()
                .map(|p| ProviderEntry::Known(ProviderSettings::new(p, p != ProviderId::Copilot)))
                .collect(),
            ..Settings::default()
        };

        let mut services: Vec<DemoService> =
            [(claude, claude_at), (codex, codex_at), (meshy, meshy_at)]
                .into_iter()
                .map(|(snapshot, at)| DemoService {
                    account: AccountRef::primary(),
                    snapshot,
                    status: fresh(at),
                })
                .collect();
        match scenario {
            Scenario::Stale => {
                let claude = &mut services[0].status;
                claude.last_failure = Some((plus(now, -1), FailureKind::NoData));
                claude.failure_detail = Some("HTTP 503".to_owned());
                claude.next_try = Some(plus(now, 1));
            }
            Scenario::SignIn => {
                services[1].status.last_failure = Some((plus(now, -1), FailureKind::Auth));
            }
            Scenario::Normal | Scenario::Warning => {}
        }

        Demo {
            now,
            settings,
            locale: LocaleFormat::en_gb(zone()),
            language: Language::English,
            theme: ThemeInfo {
                taskbar_dark: dark,
                apps_dark: dark,
                accent: AccentShades {
                    accent: Argb(0xFFB0_5CC8),
                    dark1: Argb(0xFF8E_3AA7),
                    dark2: Argb(0xFF6E_2687),
                    light2: Argb(0xFFD5_8AE8),
                    light3: Argb(0xFFE8_B7F2),
                },
                transparency: false,
                high_contrast: None,
            },
            services,
            history,
        }
    }

    /// The same world in `language`, with its regional format: sv-SE for Swedish, en-GB
    /// otherwise.
    pub fn in_language(mut self, language: Language) -> Demo {
        self.locale = if language == Language::Swedish {
            LocaleFormat::sv_se(zone())
        } else {
            LocaleFormat::en_gb(zone())
        };
        self.language = language;
        self
    }

    /// Runs `stats::evaluate` over the demo services, with every meter named in the demo's
    /// language (as the app does).
    pub fn evaluate(&self) -> Evaluation {
        let inputs: Vec<ServiceInput<'_>> = self
            .services
            .iter()
            .map(|s| ServiceInput {
                provider: s.snapshot.provider,
                account: &s.account,
                snapshot: Some(&s.snapshot),
                status: &s.status,
                fetching: false,
            })
            .collect();
        let mut eval = evaluate(
            &EvalInput {
                now: self.now,
                settings: &self.settings,
                services: &inputs,
                history: &self.history,
            },
            &mut EvalMemory::default(),
        );
        localize(&mut eval, self.language.strings());
        eval
    }

    /// The text and build context at `now`.
    pub fn build_context(&self) -> BuildContext<'_> {
        BuildContext {
            text: TextContext {
                now: self.now,
                time_style: TimeStyle::Clock,
                locale: &self.locale,
                strings: self.language.strings(),
            },
            settings: &self.settings,
        }
    }

    /// The flyout model (F§3–F§10).
    pub fn flyout(&self) -> FlyoutModel {
        let eval = self.evaluate();
        build_flyout(&eval, &self.history, false, &self.build_context())
    }

    /// The hover card over the readout's auto rows (R§8).
    pub fn hover_card(&self) -> HoverCardModel {
        let eval = self.evaluate();
        build_hover_card(
            &eval,
            HoverAnchor::Readout,
            &eval.readout_rows,
            &self.build_context(),
        )
    }

    /// The settings with a second, invented Claude account (`Work`, `~\.claude-work`), and what
    /// the settings window's accounts section knows of the invented folders: who is signed in to
    /// each and an unused `~\.claude-old` found on the PC (`--preview-settings`, accounts-spec
    /// §7). Addresses are under `example.invalid`.
    pub fn accounts(&self) -> (Settings, crate::settings_accounts::AccountsInfo) {
        use crate::settings_accounts::{AccountsInfo, folder_id};
        use lastdrop_core::model::{AccountIdentity, Identity};
        use lastdrop_core::secret::SecretString;
        use lastdrop_core::settings::{AccountSettings, SourceSettings};
        use lastdrop_providers::discover::Candidate;

        let home = std::path::PathBuf::from(r"C:\Users\tester");
        let signed_in = |name: &str, email: &str| Identity::SignedIn {
            id: AccountIdentity::of(&["claude", "org", name]),
            email: Some(SecretString::new(email.to_owned())),
        };
        let mut settings = self.settings.clone();
        let work = SourceSettings::ClaudeConfigDir {
            path: home.join(".claude-work"),
        };
        if let Some(p) = settings.provider_mut(ProviderId::Claude) {
            let taken: Vec<AccountKey> = p.accounts.iter().map(|a| a.key.clone()).collect();
            p.accounts
                .push(AccountSettings::for_folder(work, Some("Work"), &taken));
        }
        let mut info = AccountsInfo {
            home: Some(home.clone()),
            ..AccountsInfo::default()
        };
        info.defaults
            .insert(ProviderId::Claude, home.join(".claude"));
        info.defaults.insert(ProviderId::Codex, home.join(".codex"));
        for (folder, identity) in [
            (".claude", signed_in("me", "tester@example.invalid")),
            (
                ".claude-work",
                signed_in("work", "tester@work.example.invalid"),
            ),
            (".claude-old", Identity::SignedOut),
            (".codex", Identity::Unknown),
        ] {
            info.identities
                .insert(folder_id(&home.join(folder)), identity);
        }
        info.found.insert(
            ProviderId::Claude,
            vec![Candidate {
                folder: home.join(".claude-old"),
                source: SourceSettings::ClaudeConfigDir {
                    path: home.join(".claude-old"),
                },
            }],
        );
        (settings, info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lastdrop_core::flyout::{CardBody, RightKind};

    #[test]
    fn matches_the_mock_reference_content() {
        let demo = Demo::new(false);
        let flyout = demo.flyout();
        assert_eq!(flyout.verdict.text, "All limits last until their reset");
        assert_eq!(flyout.verdict.glyph, None);
        let names: Vec<&str> = flyout.cards.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Claude", "Codex", "Meshy"]);
        let claude = &flyout.cards[0];
        assert_eq!(claude.plan.as_deref(), Some("Max"));
        assert_eq!(claude.age, "1 min ago");
        let CardBody::Meters(rows) = &claude.body else {
            panic!("Claude has window meters");
        };
        let shown: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.value.as_str(), r.right.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("5-hour session", "42%", "Resets 14:56"),
                ("Weekly · all models", "61%", "Resets Mon 09:00"),
                ("Weekly · Fable", "18%", "Resets Mon 09:00"),
            ]
        );
        assert!(
            rows.iter()
                .all(|r| r.right.kind == RightKind::Reset && r.line.is_none())
        );
        assert_eq!(flyout.cards[1].age, "2 min ago");
        let CardBody::Meters(codex) = &flyout.cards[1].body else {
            panic!("Codex has window meters");
        };
        assert_eq!(codex[0].right.text, "Resets Fri 14:26");
        let CardBody::Balance(b) = &flyout.cards[2].body else {
            panic!("Meshy is a balance");
        };
        assert_eq!(b.amount, "1,240");
        assert_eq!(b.forecast, "≈ 9 days left");
        assert_eq!(b.spent, "62% of last top-up used · 136 a day");
        assert_eq!(b.until.as_deref(), Some("until ≈ Mon 19 Oct"));
        // The sparklines follow the mock's days.
        let today = |i: usize| flyout.cards[i].spark.values[13].map(f64::round);
        assert_eq!(today(0), Some(6.0));
        assert_eq!(today(1), Some(1.0));
        assert_eq!(today(2), Some(140.0));
    }

    #[test]
    fn scenarios_show_their_trouble() {
        let warning = Demo::scenario(false, Scenario::Warning).flyout();
        assert_eq!(warning.verdict.text, "Claude 5-hour runs out at 15:18");
        let CardBody::Meters(rows) = &warning.cards[0].body else {
            panic!("Claude has window meters");
        };
        assert_eq!(rows[0].right.kind, RightKind::Say);
        assert_eq!(rows[0].right.text, "Out 15:18");
        assert!(rows[0].line.is_some());

        let stale = Demo::scenario(false, Scenario::Stale).flyout();
        assert_eq!(stale.verdict.text, "Claude: no reading for 47 min");
        let message = stale.cards[0].message.as_ref().expect("message line");
        assert_eq!(message.lead, "Not answering");
        assert_eq!(message.text, " · showing 12:42 · next try 13:30");

        let sign_in = Demo::scenario(false, Scenario::SignIn).flyout();
        let message = sign_in.cards[1].message.as_ref().expect("message line");
        assert_eq!(message.lead, "Sign-in expired.");
        assert_eq!(sign_in.focus_card, Some(1));
        assert_eq!(Scenario::parse("Warn"), Some(Scenario::Warning));
        assert_eq!(Scenario::parse("nope"), None);
    }

    #[test]
    fn hover_card_is_calm() {
        let card = Demo::new(true).hover_card();
        assert_eq!(card.header.title, "You'll make it to both resets");
        assert!(!card.services.is_empty());
    }

    #[test]
    fn the_mock_in_swedish() {
        // Swedish words with sv-SE numbers and times (`42 %` and `1 240` with U+202F, `mån`);
        // meter names say "limit" themselves, and a sentence that starts with one is capitalised.
        let sv = |scenario| Demo::scenario(false, scenario).in_language(Language::Swedish);
        let flyout = sv(Scenario::Normal).flyout();
        assert_eq!(
            flyout.verdict.text,
            "Alla gränser räcker till återställningen"
        );
        assert_eq!(flyout.settings_label, "Inställningar");
        let claude = &flyout.cards[0];
        assert_eq!(claude.age, "för 1 min sedan");
        let CardBody::Meters(rows) = &claude.body else {
            panic!("Claude has window meters");
        };
        let shown: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.value.as_str(), r.right.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("5-timmarssession", "42\u{202F}%", "Återställs 14:56"),
                (
                    "Vecka · alla modeller",
                    "61\u{202F}%",
                    "Återställs mån 09:00"
                ),
                ("Vecka · Fable", "18\u{202F}%", "Återställs mån 09:00"),
            ]
        );
        let CardBody::Balance(b) = &flyout.cards[2].body else {
            panic!("Meshy is a balance");
        };
        assert_eq!(b.amount, "1\u{202F}240");
        assert_eq!(b.forecast, "≈ 9 dagar kvar");
        assert_eq!(b.spent, "62\u{202F}% av påfyllningen använt · 136/dag");
        assert_eq!(b.until.as_deref(), Some("till ≈ mån 19 okt"));

        let warning = sv(Scenario::Warning).flyout();
        assert_eq!(
            warning.verdict.text,
            "5-timmarsgränsen för Claude tar slut 15:18"
        );
        let CardBody::Meters(rows) = &warning.cards[0].body else {
            panic!("Claude has window meters");
        };
        assert_eq!(rows[0].right.text, "Slut 15:18");
        let stale = sv(Scenario::Stale).flyout();
        assert_eq!(stale.verdict.text, "Claude: ingen avläsning på 47 min");

        let card = sv(Scenario::Normal).hover_card();
        assert_eq!(card.header.title, "Det räcker till båda återställningarna");
        let also = card.also.expect("also tracked");
        assert_eq!(also.header, "Bevakas också");
        let labels: Vec<&str> = also.lines.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Fable-veckogränsen för Claude",
                "Veckogränsen för Codex",
                "Krediterna för Meshy"
            ]
        );
        let card = sv(Scenario::Warning).hover_card();
        assert_eq!(card.header.title, "5-timmarsgränsen tar slut 15:18");
    }
}
