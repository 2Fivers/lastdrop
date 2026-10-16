//! Notifications (N§, `docs/design/notifications-spec.md`): which notices an evaluation raises and
//! the text of the one Windows notification they become.
//!
//! [`notices`] runs after every [`crate::stats::evaluate`] with the memory of what was announced
//! ([`NotifyMemory`], which the app keeps across restarts in its state file) and returns only what
//! is new; [`build_notification`] words them. The app shows the result with the tray icon
//! (`lastdrop_win::tray::Tray::notify`, N§4).

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::consts::{NOTIFY_FORGET_AFTER, NOTIFY_MAX_LISTED, NOTIFY_USED_UP_LEVEL};
use crate::grammar::a11y::unit_word;
use crate::grammar::numbers::{amount, balance_duration_words, percent};
use crate::grammar::strings::text;
use crate::grammar::time::{card_time, long_duration, shown_duration};
use crate::grammar::{Msg, TextContext};
use crate::model::MeterKey;
use crate::readout::hovercard::before_reset;
use crate::settings::NotificationSettings;
use crate::stats::{
    AssessedMeter, Assessment, Evaluation, Freshness, LiveChange, ResetView, ServiceEval, Severity,
    StaleReason, WindowAssessment, same_window_instance,
};
use crate::util::sub;

/// What one notice says (N§1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeKind {
    /// LEVEL: a window meter's `pct` reached this level, the highest one it reached.
    Level(u8),
    /// USED_UP: a window meter is used up.
    UsedUp,
    /// RESET: a window reset after a level or used up was announced in it; `used_up` when used
    /// up had been.
    Reset {
        /// Used up was announced in the window that ended.
        used_up: bool,
    },
    /// BALANCE_LOW: a balance's severity rose to WARNING or CRITICAL.
    BalanceLow(Severity),
    /// BALANCE_EMPTY: a balance is empty.
    BalanceEmpty,
}

impl NoticeKind {
    /// Order of importance (N§3), most important first.
    fn rank(self) -> u8 {
        match self {
            NoticeKind::UsedUp | NoticeKind::BalanceEmpty => 0,
            NoticeKind::Level(_) => 1,
            NoticeKind::BalanceLow(s) if s >= Severity::Critical => 1,
            NoticeKind::BalanceLow(_) => 2,
            NoticeKind::Reset { .. } => 3,
        }
    }

    /// Shown with the warning icon; a reset is good news and gets the information icon (N§4).
    pub fn is_warning(self) -> bool {
        !matches!(self, NoticeKind::Reset { .. })
    }
}

/// One notice: a meter and what is said about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    /// The meter.
    pub key: MeterKey,
    /// What is said.
    pub kind: NoticeKind,
}

/// What was announced for one meter (N§1 "Once per").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Announced {
    /// A window meter: the highest level announced in the window instance that ends at
    /// `instance` (its `R`; `None` for a window without reset times); [`NOTIFY_USED_UP_LEVEL`]
    /// when used up was.
    Window {
        /// `R` of the window instance.
        instance: Option<Timestamp>,
        /// The level announced.
        level: u8,
    },
    /// A balance meter: the highest severity announced since it was last OK.
    Balance {
        /// The severity announced.
        severity: Severity,
    },
}

/// What was announced, per meter. Owned by the app's state (main thread) and saved in its state
/// file, so a restart repeats nothing (N§1).
pub type NotifyMemory = BTreeMap<MeterKey, Announced>;

/// The notices `eval` raises that `memory` has not announced yet, most important first (N§1,
/// N§3), and `memory` updated with them. Nothing when notifications are off (the memory is left
/// as it is).
///
/// Order of work: resets first (from `eval.changes`, reading what the window that ended
/// announced), then every meter in service and meter order, then announced windows whose reset
/// is more than [`NOTIFY_FORGET_AFTER`] past are forgotten.
pub fn notices(
    eval: &Evaluation,
    settings: &NotificationSettings,
    memory: &mut NotifyMemory,
) -> Vec<Notice> {
    if !settings.enabled {
        return Vec::new();
    }
    let levels = settings.levels();
    let mut out = Vec::new();

    for change in &eval.changes {
        let LiveChange::WindowReset(key) = change else {
            continue;
        };
        let Some(Announced::Window { level, .. }) = memory.get(key).copied() else {
            continue;
        };
        memory.remove(key);
        let paused = eval.services.iter().any(|s| {
            s.source.provider == key.provider
                && s.source.account == key.account
                && s.freshness == Freshness::Stale(StaleReason::Paused)
        });
        if settings.resets && !paused {
            out.push(Notice {
                key: key.clone(),
                kind: NoticeKind::Reset {
                    used_up: level >= NOTIFY_USED_UP_LEVEL,
                },
            });
        }
    }

    for a in eval.services.iter().flat_map(|s| s.meters.iter()) {
        let notice = match &a.meter {
            AssessedMeter::Window(w) => window_notice(a, w, &levels, settings, memory),
            AssessedMeter::Balance(_) => balance_notice(a, settings, memory),
            AssessedMeter::Loading => None,
        };
        out.extend(notice);
    }

    let forget_before = sub(eval.now, NOTIFY_FORGET_AFTER);
    memory.retain(|_, announced| match announced {
        Announced::Window {
            instance: Some(r), ..
        } => *r >= forget_before,
        _ => true,
    });
    // Stable: equal ranks stay in reset-then-service-and-meter order.
    out.sort_by_key(|n| n.kind.rank());
    out
}

/// LEVEL or USED_UP for a fresh, live window meter (N§1): the highest level its `pct` reached
/// when that is above what its window instance announced.
fn window_notice(
    a: &Assessment,
    w: &WindowAssessment,
    levels: &[u8],
    settings: &NotificationSettings,
    memory: &mut NotifyMemory,
) -> Option<Notice> {
    // Stale and ended meters have no severity; a window drawn as reset shows `u = 0`.
    if a.severity.is_none() || w.reset != ResetView::Live {
        return None;
    }
    let instance = w.resets_at;
    let announced = match memory.get(&a.key) {
        Some(Announced::Window { instance: i, level }) if same_window_instance(*i, instance) => {
            *level
        }
        _ => 0,
    };
    let reached = if w.exhausted && settings.used_up {
        NOTIFY_USED_UP_LEVEL
    } else {
        levels
            .iter()
            .rev()
            .copied()
            .find(|&l| w.pct >= l)
            .unwrap_or(0)
    };
    if reached <= announced {
        if announced == 0 {
            // What another instance announced (one that ended while Lastdrop was not running, so
            // no evaluation saw its reset): forgotten without a RESET.
            memory.remove(&a.key);
        }
        return None;
    }
    memory.insert(
        a.key.clone(),
        Announced::Window {
            instance,
            level: reached,
        },
    );
    let kind = if reached >= NOTIFY_USED_UP_LEVEL {
        NoticeKind::UsedUp
    } else {
        NoticeKind::Level(reached)
    };
    Some(Notice {
        key: a.key.clone(),
        kind,
    })
}

/// BALANCE_LOW or BALANCE_EMPTY for a fresh balance meter whose severity rose above what was
/// announced since it was last OK (N§1). OK forgets what was announced.
fn balance_notice(
    a: &Assessment,
    settings: &NotificationSettings,
    memory: &mut NotifyMemory,
) -> Option<Notice> {
    let severity = a.severity?;
    if severity == Severity::Ok {
        memory.remove(&a.key);
        return None;
    }
    let announced = match memory.get(&a.key) {
        Some(Announced::Balance { severity }) => *severity,
        _ => Severity::Ok,
    };
    if severity <= announced {
        return None;
    }
    let kind = match severity {
        Severity::Exhausted if settings.used_up => NoticeKind::BalanceEmpty,
        Severity::Warning | Severity::Critical if settings.low_balance => {
            NoticeKind::BalanceLow(severity)
        }
        _ => return None,
    };
    memory.insert(a.key.clone(), Announced::Balance { severity });
    Some(Notice {
        key: a.key.clone(),
        kind,
    })
}

/// One Windows notification (N§3, N§4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationModel {
    /// The title: the most important notice's.
    pub title: String,
    /// The text: that notice's sentence, then up to [`NOTIFY_MAX_LISTED`] other notices' titles
    /// and `+{n} more`, one per line.
    pub body: String,
    /// The warning icon; else the information icon (only resets).
    pub warning: bool,
}

/// The notification for `notices` (most important first, as [`notices`] returns them), worded
/// at `cx.now`; `None` when there is nothing to say. A notice whose meter is not in `eval` is
/// left out.
pub fn build_notification(
    notices: &[Notice],
    eval: &Evaluation,
    cx: &TextContext<'_>,
) -> Option<NotificationModel> {
    let worded: Vec<(NoticeKind, String, &ServiceEval, &Assessment)> = notices
        .iter()
        .filter_map(|n| {
            let (service, a) = eval.assessment(&n.key)?;
            Some((n.kind, title(n.kind, service, a, cx), service, a))
        })
        .collect();
    let ((kind, first_title, service, a), rest) = worded.split_first()?;
    let mut lines = vec![body(*kind, service, a, cx)];
    lines.extend(
        rest.iter()
            .take(NOTIFY_MAX_LISTED)
            .map(|(_, t, _, _)| t.clone()),
    );
    if rest.len() > NOTIFY_MAX_LISTED {
        let more = (rest.len() - NOTIFY_MAX_LISTED).to_string();
        lines.push(text(cx.strings, Msg::NoticeMore, &[("n", &more)]));
    }
    Some(NotificationModel {
        title: first_title.clone(),
        body: lines.join("\n"),
        warning: worded.iter().any(|(k, _, _, _)| k.is_warning()),
    })
}

/// A notice's title (N§3): `Claude weekly limit is 82% used`.
fn title(kind: NoticeKind, service: &ServiceEval, a: &Assessment, cx: &TextContext<'_>) -> String {
    let msg = match kind {
        NoticeKind::Level(_) => Msg::NoticeLevelTitle,
        NoticeKind::UsedUp => Msg::NoticeUsedUpTitle,
        NoticeKind::Reset { .. } => Msg::NoticeResetTitle,
        NoticeKind::BalanceLow(_) => Msg::NoticeBalanceLowTitle,
        NoticeKind::BalanceEmpty => Msg::NoticeBalanceEmptyTitle,
    };
    let pct = match &a.meter {
        AssessedMeter::Window(w) => w.pct,
        AssessedMeter::Balance(b) => b.pct,
        AssessedMeter::Loading => 0,
    };
    text(
        cx.strings,
        msg,
        &[
            ("service", &service.display_name()),
            ("meter", &a.names.short_name),
            ("pct", &percent(pct, cx.locale)),
        ],
    )
}

/// A notice's sentence (N§3).
fn body(kind: NoticeKind, service: &ServiceEval, a: &Assessment, cx: &TextContext<'_>) -> String {
    let s = cx.strings;
    let no_reset = || s.template(Msg::NoticeBodyNoReset).to_owned();
    match (&a.meter, kind) {
        (AssessedMeter::Window(w), NoticeKind::Reset { used_up }) => {
            let msg = if used_up {
                Msg::NoticeBodyResetUsedUp
            } else {
                Msg::NoticeBodyReset
            };
            text(s, msg, &[("pct", &percent(w.pct, cx.locale))])
        }
        (AssessedMeter::Window(w), NoticeKind::UsedUp) => match w.resets_at {
            Some(r) => text(
                s,
                Msg::NoticeBodyBack,
                &[
                    ("time", &card_time(r, cx)),
                    ("duration", &long_duration(shown_duration(cx.now, r), s)),
                ],
            ),
            None => no_reset(),
        },
        (AssessedMeter::Window(w), _) => {
            let forecast = w.forecast.filter(|f| f.valid);
            match (w.resets_at, forecast) {
                (Some(r), Some(f)) if f.runs_out => match f.t_out {
                    Some(t) => text(
                        s,
                        Msg::NoticeBodyRunsOut,
                        &[
                            ("time", &card_time(t, cx)),
                            ("duration", &before_reset(t, r, cx)),
                        ],
                    ),
                    None => text(s, Msg::NoticeBodyResets, &[("time", &card_time(r, cx))]),
                },
                (Some(r), Some(_)) => text(s, Msg::NoticeBodyLasts, &[("time", &card_time(r, cx))]),
                (Some(r), None) => text(s, Msg::NoticeBodyResets, &[("time", &card_time(r, cx))]),
                (None, _) => no_reset(),
            }
        }
        (AssessedMeter::Balance(_), NoticeKind::BalanceEmpty) => text(
            s,
            Msg::NoticeBodyBalanceEmpty,
            &[("service", &service.display_name())],
        ),
        (AssessedMeter::Balance(b), _) => {
            let left = amount(b.amount, cx.locale);
            let unit = unit_word(&b.unit, s);
            match balance_duration_words(&b.stats, s) {
                Some(words) => text(
                    s,
                    Msg::NoticeBodyBalance,
                    &[("amount", &left), ("unit", &unit), ("duration", &words)],
                ),
                None => text(
                    s,
                    Msg::NoticeBodyBalanceLeft,
                    &[("amount", &left), ("unit", &unit)],
                ),
            }
        }
        (AssessedMeter::Loading, _) => no_reset(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountKey, MeterId, ProviderId};

    fn key(meter: &str) -> MeterKey {
        MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::new(meter),
        }
    }

    #[test]
    fn used_up_and_empty_come_first_and_resets_last() {
        let mut kinds = vec![
            NoticeKind::Reset { used_up: true },
            NoticeKind::BalanceLow(Severity::Warning),
            NoticeKind::Level(80),
            NoticeKind::BalanceLow(Severity::Critical),
            NoticeKind::UsedUp,
            NoticeKind::BalanceEmpty,
        ];
        kinds.sort_by_key(|k| k.rank());
        assert_eq!(
            kinds,
            vec![
                NoticeKind::UsedUp,
                NoticeKind::BalanceEmpty,
                NoticeKind::Level(80),
                NoticeKind::BalanceLow(Severity::Critical),
                NoticeKind::BalanceLow(Severity::Warning),
                NoticeKind::Reset { used_up: true },
            ]
        );
        assert!(!NoticeKind::Reset { used_up: false }.is_warning());
        assert!(NoticeKind::Level(95).is_warning());
    }

    #[test]
    fn the_memory_round_trips_as_json() {
        let r = Timestamp::from_second(1_800_000_000).expect("valid");
        let memory: NotifyMemory = BTreeMap::from([
            (
                key(MeterId::WEEKLY),
                Announced::Window {
                    instance: Some(r),
                    level: 95,
                },
            ),
            (
                key(MeterId::CREDITS),
                Announced::Balance {
                    severity: Severity::Warning,
                },
            ),
        ]);
        let entries: Vec<(MeterKey, Announced)> = memory.clone().into_iter().collect();
        let json = serde_json::to_string(&entries).expect("serialize");
        let back: Vec<(MeterKey, Announced)> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.into_iter().collect::<NotifyMemory>(), memory);
    }
}
