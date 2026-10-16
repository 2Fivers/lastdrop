//! Notifications (notifications-spec N§1–N§3): snapshots → `stats::evaluate` → `notify::notices`
//! → `notify::build_notification`, over several evaluations as the app runs them.
//!
//! All data is invented. "Local" is a fixed UTC+02:00 zone, en-US format; `now` is Sat 2026-10-10
//! 12:42 unless a test says otherwise (as in R§13).

use std::collections::BTreeMap;

use jiff::tz::{self, TimeZone};
use jiff::{SignedDuration, Timestamp, civil};
use lastdrop_core::grammar::{English, LocaleFormat, TextContext, TimeStyle};
use lastdrop_core::model::{
    AccountKey, AccountRef, Balance, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId,
    Snapshot, SourceKind, Unit, WindowLen,
};
use lastdrop_core::notify::{
    Announced, Notice, NoticeKind, NotificationModel, NotifyMemory, build_notification, notices,
};
use lastdrop_core::settings::{Pause, ProviderSettings, Settings};
use lastdrop_core::stats::{
    self, EvalInput, EvalMemory, Evaluation, Sample, ServiceInput, ServiceStatus, Severity,
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

fn five_hour(used: f64, resets_at: Option<Timestamp>) -> Meter {
    Meter {
        id: MeterId::new(MeterId::FIVE_HOUR),
        names: MeterNames::new("5h", "5-hour", "5-hour session"),
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
        names: MeterNames::new("Wk", "weekly", "Weekly · all models"),
        order: 1,
        used,
        limited: false,
        resets_at,
        window: WindowLen::Fixed(hours(7 * 24)),
        starts_on_use: false,
    }
}

fn claude(meters: Vec<Meter>, t: Timestamp) -> Snapshot {
    Snapshot {
        provider: ProviderId::Claude,
        account: AccountRef::primary(),
        source: SourceKind::ClaudeOAuthUsage,
        measured_at: t,
        fetched_at: t,
        meters,
        balances: Vec::new(),
        health: Health::Ok,
    }
}

fn meshy(amount: f64, t: Timestamp) -> Snapshot {
    Snapshot {
        provider: ProviderId::Meshy,
        source: SourceKind::MeshyApi,
        meters: Vec::new(),
        balances: vec![Balance {
            id: MeterId::new(MeterId::CREDITS),
            names: MeterNames::new("Cr", "credits", "Credits"),
            order: 0,
            amount,
            unit: Unit::Credits,
            last_top_up: None,
        }],
        ..claude(Vec::new(), t)
    }
}

fn key(provider: ProviderId, meter: &str) -> MeterKey {
    MeterKey {
        provider,
        account: AccountKey::primary(),
        meter: MeterId::new(meter),
    }
}

fn weekly_key() -> MeterKey {
    key(ProviderId::Claude, MeterId::WEEKLY)
}

fn five_hour_key() -> MeterKey {
    key(ProviderId::Claude, MeterId::FIVE_HOUR)
}

fn credits_key() -> MeterKey {
    key(ProviderId::Meshy, MeterId::CREDITS)
}

fn settings() -> Settings {
    let mut settings = Settings::default();
    settings.providers = [ProviderId::Claude, ProviderId::Meshy]
        .into_iter()
        .map(|p| lastdrop_core::settings::ProviderEntry::Known(ProviderSettings::new(p, true)))
        .collect();
    settings
}

fn locale() -> LocaleFormat {
    LocaleFormat::en_us(zone())
}

type History = BTreeMap<MeterKey, Vec<Sample>>;

/// The app's loop: evaluation memory, notification memory and a history, evaluated again and
/// again.
struct World {
    settings: Settings,
    memory: EvalMemory,
    announced: NotifyMemory,
    history: History,
}

impl World {
    fn new() -> World {
        World {
            settings: settings(),
            memory: EvalMemory::default(),
            announced: NotifyMemory::new(),
            history: History::new(),
        }
    }

    /// Evaluates the readings at `now` (each fresh as of its own `t_r`, or with `status`) and
    /// returns the evaluation and its notices.
    fn run_with(
        &mut self,
        readings: &[(Snapshot, ServiceStatus)],
        now: Timestamp,
    ) -> (Evaluation, Vec<Notice>) {
        let accounts: Vec<AccountRef> = readings.iter().map(|(s, _)| s.account.clone()).collect();
        let inputs: Vec<ServiceInput<'_>> = readings
            .iter()
            .zip(&accounts)
            .map(|((snapshot, status), account)| ServiceInput {
                provider: snapshot.provider,
                account,
                snapshot: Some(snapshot),
                status,
                fetching: false,
            })
            .collect();
        let eval = stats::evaluate(
            &EvalInput {
                now,
                settings: &self.settings,
                services: &inputs,
                history: &self.history,
            },
            &mut self.memory,
        );
        let found = notices(&eval, &self.settings.notifications, &mut self.announced);
        (eval, found)
    }

    fn run(&mut self, readings: &[Snapshot], now: Timestamp) -> (Evaluation, Vec<Notice>) {
        let with_status: Vec<(Snapshot, ServiceStatus)> = readings
            .iter()
            .map(|s| (s.clone(), fresh(s.measured_at)))
            .collect();
        self.run_with(&with_status, now)
    }

    fn kinds(&mut self, readings: &[Snapshot], now: Timestamp) -> Vec<(MeterKey, NoticeKind)> {
        let (_, found) = self.run(readings, now);
        found.into_iter().map(|n| (n.key, n.kind)).collect()
    }

    /// A restart: the evaluation memory starts empty, the notification memory is what the state
    /// file kept.
    fn restart(&mut self) {
        self.memory = EvalMemory::default();
    }
}

fn fresh(t: Timestamp) -> ServiceStatus {
    ServiceStatus {
        last_success: Some(t),
        poll_interval: Some(mins(5)),
        ..ServiceStatus::default()
    }
}

fn worded(eval: &Evaluation, found: &[Notice]) -> NotificationModel {
    let locale = locale();
    let cx = TextContext {
        now: eval.now,
        time_style: TimeStyle::Clock,
        locale: &locale,
        strings: &English,
    };
    build_notification(found, eval, &cx).expect("a notification")
}

// ---- LEVEL and USED_UP ----------------------------------------------------------------------------

#[test]
fn each_level_is_announced_once_per_window() {
    let r = at(10, 12, 9, 0);
    let mut world = World::new();
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.42, Some(r))], now())], now()),
        []
    );

    let t = now() + mins(30);
    let (eval, found) = world.run(&[claude(vec![weekly(0.82, Some(r))], t)], t);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, NoticeKind::Level(80));
    let n = worded(&eval, &found);
    assert_eq!(n.title, "Claude weekly limit is 82% used");
    assert_eq!(n.body, "It resets Mon 9:00 AM.");
    assert!(n.warning);

    // Still past 80 %, a minute later and after a provider correction: nothing new.
    let t = t + mins(1);
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.83, Some(r))], t)], t),
        []
    );
    let t = t + mins(5);
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.79, Some(r))], t)], t),
        []
    );
    let t = t + mins(5);
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.81, Some(r))], t)], t),
        []
    );

    let t = t + mins(60);
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.95, Some(r))], t)], t),
        [(weekly_key(), NoticeKind::Level(95))]
    );
    // Its reset jitters by seconds: the same window.
    let t = t + mins(5);
    let jittered = r + SignedDuration::from_secs(40);
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.97, Some(jittered))], t)], t),
        []
    );
}

#[test]
fn several_levels_at_once_give_the_highest_and_used_up_tops_them() {
    let r = at(10, 10, 16, 30);
    let mut world = World::new();
    assert_eq!(
        world.kinds(&[claude(vec![five_hour(0.96, Some(r))], now())], now()),
        [(five_hour_key(), NoticeKind::Level(95))]
    );
    let t = now() + mins(10);
    let (eval, found) = world.run(&[claude(vec![five_hour(1.0, Some(r))], t)], t);
    assert_eq!(found[0].kind, NoticeKind::UsedUp);
    let n = worded(&eval, &found);
    assert_eq!(n.title, "Claude 5-hour limit is used up");
    assert_eq!(n.body, "It is back at 4:30 PM, in 3 h 38 min.");
}

#[test]
fn the_text_follows_the_forecast() {
    let r = at(10, 10, 16, 30);
    let mut world = World::new();
    let tn = at(10, 10, 13, 45);
    world.history.insert(
        five_hour_key(),
        vec![
            Sample::window(at(10, 10, 13, 15), 0.71, Some(r)),
            Sample::window(tn, 0.80, Some(r)),
        ],
    );
    let (eval, found) = world.run(&[claude(vec![five_hour(0.80, Some(r))], tn)], tn);
    assert_eq!(
        worded(&eval, &found).body,
        "At your recent pace it runs out at 2:52 PM, 1 h 38 min before the reset."
    );

    let mut world = World::new();
    let tn = at(10, 10, 16, 5);
    world.history.insert(
        five_hour_key(),
        vec![
            Sample::window(at(10, 10, 15, 30), 0.92, Some(r)),
            Sample::window(tn, 0.92, Some(r)),
        ],
    );
    let (eval, found) = world.run(&[claude(vec![five_hour(0.92, Some(r))], tn)], tn);
    assert_eq!(
        worded(&eval, &found).body,
        "At your recent pace it lasts until the reset at 4:30 PM."
    );
}

#[test]
fn used_up_off_announces_the_highest_level_instead() {
    let mut world = World::new();
    world.settings.notifications.used_up = false;
    let r = at(10, 10, 16, 30);
    assert_eq!(
        world.kinds(&[claude(vec![five_hour(1.0, Some(r))], now())], now()),
        [(five_hour_key(), NoticeKind::Level(95))]
    );
}

#[test]
fn levels_from_the_settings_are_sanitized() {
    let mut world = World::new();
    world.settings.notifications.levels = vec![90, 0, 50, 120, 50];
    assert_eq!(world.settings.notifications.levels(), [50, 90]);
    let r = at(10, 12, 9, 0);
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.6, Some(r))], now())], now()),
        [(weekly_key(), NoticeKind::Level(50))]
    );
}

// ---- RESET --------------------------------------------------------------------------------------

#[test]
fn a_window_that_was_announced_announces_its_reset() {
    let r = at(10, 10, 16, 30);
    let mut world = World::new();
    let t = at(10, 10, 16, 0);
    let reading = claude(vec![five_hour(1.0, Some(r))], t);
    assert_eq!(
        world.kinds(std::slice::from_ref(&reading), t),
        [(five_hour_key(), NoticeKind::UsedUp)]
    );
    // The reset passes with no newer reading (R§3.9: a starts-on-use window goes idle).
    let t = at(10, 10, 16, 31);
    let (eval, found) = world.run(std::slice::from_ref(&reading), t);
    assert_eq!(found[0].kind, NoticeKind::Reset { used_up: true });
    let n = worded(&eval, &found);
    assert_eq!(n.title, "Claude 5-hour limit has reset");
    assert_eq!(n.body, "Back to 0% used. You can use it again.");
    assert!(!n.warning);
    // Announced once; the next window starts from nothing.
    assert_eq!(world.kinds(&[reading], at(10, 10, 16, 32)), []);
    assert!(world.announced.is_empty());
}

#[test]
fn a_fixed_window_reset_after_a_level_says_back_to_zero() {
    let r = at(10, 12, 9, 0);
    let mut world = World::new();
    let reading = claude(vec![weekly(0.85, Some(r))], at(10, 12, 8, 0));
    assert_eq!(
        world.kinds(std::slice::from_ref(&reading), at(10, 12, 8, 0)),
        [(weekly_key(), NoticeKind::Level(80))]
    );
    let (eval, found) = world.run(&[reading], at(10, 12, 9, 1));
    assert_eq!(found[0].kind, NoticeKind::Reset { used_up: false });
    assert_eq!(worded(&eval, &found).body, "Back to 0% used.");
}

#[test]
fn a_window_never_announced_resets_quietly() {
    let r = at(10, 10, 16, 30);
    let mut world = World::new();
    let reading = claude(vec![five_hour(0.5, Some(r))], at(10, 10, 16, 0));
    assert_eq!(
        world.kinds(std::slice::from_ref(&reading), at(10, 10, 16, 0)),
        []
    );
    assert_eq!(world.kinds(&[reading], at(10, 10, 16, 31)), []);
}

#[test]
fn resets_off_or_paused_say_nothing_but_forget_the_window() {
    for paused in [false, true] {
        let r = at(10, 10, 16, 30);
        let mut world = World::new();
        let t = at(10, 10, 16, 0);
        let reading = claude(vec![five_hour(0.9, Some(r))], t);
        assert_eq!(
            world.kinds(std::slice::from_ref(&reading), t),
            [(five_hour_key(), NoticeKind::Level(80))]
        );
        let t = at(10, 10, 16, 31);
        let status = if paused {
            world.settings.pause = Some(Pause { until: None });
            ServiceStatus {
                paused: true,
                ..fresh(reading.measured_at)
            }
        } else {
            world.settings.notifications.resets = false;
            fresh(reading.measured_at)
        };
        let (_, found) = world.run_with(&[(reading, status)], t);
        assert_eq!(found, [], "paused: {paused}");
        assert!(world.announced.is_empty(), "paused: {paused}");
    }
}

// ---- restarts and memory ------------------------------------------------------------------------

#[test]
fn a_restart_repeats_nothing() {
    let r = at(10, 12, 9, 0);
    let mut world = World::new();
    let reading = claude(vec![weekly(0.96, Some(r))], now());
    assert_eq!(
        world.kinds(std::slice::from_ref(&reading), now()),
        [(weekly_key(), NoticeKind::Level(95))]
    );
    world.restart();
    assert_eq!(world.kinds(&[reading], now() + mins(3)), []);
}

#[test]
fn a_window_that_ended_while_lastdrop_was_closed_is_forgotten_without_a_reset() {
    let r_old = at(10, 10, 11, 0);
    let mut world = World::new();
    world.announced.insert(
        five_hour_key(),
        Announced::Window {
            instance: Some(r_old),
            level: 95,
        },
    );
    let r_new = at(10, 10, 16, 30);
    assert_eq!(
        world.kinds(&[claude(vec![five_hour(0.85, Some(r_new))], now())], now()),
        [(five_hour_key(), NoticeKind::Level(80))]
    );
    assert_eq!(
        world.announced.get(&five_hour_key()),
        Some(&Announced::Window {
            instance: Some(r_new),
            level: 80
        })
    );
    // Below every level in a new window: the old entry goes without a notice.
    let mut world = World::new();
    world.announced.insert(
        five_hour_key(),
        Announced::Window {
            instance: Some(r_old),
            level: 95,
        },
    );
    assert_eq!(
        world.kinds(&[claude(vec![five_hour(0.2, Some(r_new))], now())], now()),
        []
    );
    assert!(world.announced.is_empty());
}

#[test]
fn windows_of_meters_that_went_away_are_forgotten_a_day_after_their_reset() {
    let mut world = World::new();
    let gone = key(ProviderId::Claude, "weekly:fable");
    world.announced.insert(
        gone.clone(),
        Announced::Window {
            instance: Some(now() - hours(23)),
            level: 80,
        },
    );
    let r = at(10, 12, 9, 0);
    let reading = claude(vec![weekly(0.1, Some(r))], now());
    assert_eq!(world.kinds(std::slice::from_ref(&reading), now()), []);
    assert!(world.announced.contains_key(&gone));
    assert_eq!(world.kinds(&[reading], now() + hours(2)), []);
    assert!(!world.announced.contains_key(&gone));
}

#[test]
fn stale_services_and_turned_off_notifications_say_nothing() {
    let r = at(10, 12, 9, 0);
    let mut world = World::new();
    let read = now() - mins(47);
    let reading = claude(vec![weekly(0.9, Some(r))], read);
    assert_eq!(world.kinds(std::slice::from_ref(&reading), now()), []);

    world.settings.notifications.enabled = false;
    assert_eq!(
        world.kinds(&[claude(vec![weekly(0.9, Some(r))], now())], now()),
        []
    );
    assert!(world.announced.is_empty());
}

// ---- balances -----------------------------------------------------------------------------------

/// A top-up to 3,260 twenty days ago, then 952 credits spent over the last 7 days (136 a day),
/// ending at `current` at `t` (R§13's balance vector).
fn meshy_history(current: f64, t: Timestamp) -> Vec<Sample> {
    let start_of_span = current + 952.0;
    vec![
        Sample::balance(t - hours(21 * 24), 50.0),
        Sample::balance(t - hours(20 * 24), 3260.0),
        Sample::balance(t - hours(7 * 24) - mins(1), start_of_span),
        Sample::balance(t - hours(7 * 24), start_of_span),
        Sample::balance(t, current),
    ]
}

#[test]
fn a_balance_announces_each_rise_until_it_is_ok_again() {
    let mut world = World::new();
    let at_amount = |world: &mut World, amount: f64| {
        world
            .history
            .insert(credits_key(), meshy_history(amount, now()));
        world.run(&[meshy(amount, now())], now())
    };
    let (_, found) = at_amount(&mut world, 1240.0);
    assert_eq!(found, []);

    let (eval, found) = at_amount(&mut world, 620.0);
    assert_eq!(found[0].kind, NoticeKind::BalanceLow(Severity::Warning));
    let n = worded(&eval, &found);
    assert_eq!(n.title, "Meshy credits running low");
    assert_eq!(
        n.body,
        "620 credits left, about 4 days at your use of the last 7 days."
    );
    let (_, found) = at_amount(&mut world, 600.0);
    assert_eq!(found, []);

    let (_, found) = at_amount(&mut world, 200.0);
    assert_eq!(
        found.iter().map(|n| n.kind).collect::<Vec<_>>(),
        [NoticeKind::BalanceLow(Severity::Critical)]
    );
    let (eval, found) = at_amount(&mut world, 0.0);
    assert_eq!(found[0].kind, NoticeKind::BalanceEmpty);
    assert_eq!(worded(&eval, &found).title, "Meshy credits used up");
    assert_eq!(worded(&eval, &found).body, "Top it up to keep using Meshy.");

    // Topped up: OK forgets, so running low again is news.
    let (_, found) = at_amount(&mut world, 3000.0);
    assert_eq!(found, []);
    assert!(world.announced.is_empty());
    let (_, found) = at_amount(&mut world, 620.0);
    assert_eq!(
        found.iter().map(|n| n.kind).collect::<Vec<_>>(),
        [NoticeKind::BalanceLow(Severity::Warning)]
    );
}

// ---- one notification for several notices (N§3) -------------------------------------------------

#[test]
fn several_notices_become_one_notification() {
    let mut world = World::new();
    world
        .history
        .insert(credits_key(), meshy_history(620.0, now()));
    let r5 = at(10, 10, 16, 30);
    let rw = at(10, 12, 9, 0);
    let (eval, found) = world.run(
        &[
            claude(
                vec![five_hour(0.85, Some(r5)), weekly(1.0, Some(rw))],
                now(),
            ),
            meshy(620.0, now()),
        ],
        now(),
    );
    assert_eq!(
        found.iter().map(|n| n.kind).collect::<Vec<_>>(),
        [
            NoticeKind::UsedUp,
            NoticeKind::Level(80),
            NoticeKind::BalanceLow(Severity::Warning),
        ]
    );
    let n = worded(&eval, &found);
    assert_eq!(n.title, "Claude weekly limit is used up");
    assert_eq!(
        n.body,
        "It is back at Mon 9:00 AM, in 1 d 20 h.\n\
         Claude 5-hour limit is 85% used\n\
         Meshy credits running low"
    );

    // A fourth is counted, not listed.
    let extra = vec![
        found[0].clone(),
        found[1].clone(),
        found[2].clone(),
        Notice {
            key: five_hour_key(),
            kind: NoticeKind::Level(95),
        },
    ];
    assert!(worded(&eval, &extra).body.ends_with("\n+1 more"));
}
