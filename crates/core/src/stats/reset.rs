//! Window reset handling when `now ≥ R` and no newer reading has arrived (R§3.9).

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::model::{Meter, WindowLen};
use crate::util::add;

/// Bound on the resets skipped in one call (a calendar month advances one month per step; a
/// reading years old must not loop for long).
const MAX_RESET_STEPS: u32 = 1200;

/// `L` of a window when no reset time pins down which instance: fixed lengths only.
fn length_without_reset(window: WindowLen) -> Option<SignedDuration> {
    match window {
        WindowLen::Fixed(len) => Some(len),
        WindowLen::CalendarMonth | WindowLen::Unknown => None,
    }
}

/// The first reset after `now` of a fixed-schedule window whose reset `passed` is `≤ now`, and
/// the last reset that passed before it. `None` when the schedule cannot advance.
fn advance(window: WindowLen, passed: Timestamp, now: Timestamp) -> Option<(Timestamp, Timestamp)> {
    if let WindowLen::Fixed(len) = window {
        if !len.is_positive() {
            return None;
        }
        // Whole windows between `passed` and `now`, computed directly.
        let behind = now.duration_since(passed).as_secs_f64() / len.as_secs_f64();
        // `behind` is finite and ≥ 0 here; the cast saturates for absurd inputs.
        let steps = behind.floor() as i32;
        let last = passed.checked_add(len.checked_mul(steps)?).ok()?;
        let next = last.checked_add(len).ok()?;
        return Some((last, next));
    }
    let mut last = passed;
    for _ in 0..MAX_RESET_STEPS {
        let next = window.next_reset(last)?;
        if next <= last {
            return None;
        }
        if next > now {
            return Some((last, next));
        }
        last = next;
    }
    None
}

/// How a window meter is shown relative to its last reading's reset time (R§3.9, R§4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetView {
    /// `now < R`, or `R` none: the reading as it is.
    Live,
    /// Fixed-schedule window (`starts_on_use` = no, `L` known) whose `R` passed: a new window,
    /// `u = 0`, `R' = R + L` (calendar month: next month start; repeated until `R' > now`),
    /// severity OK, forecast invalid (guards). State JUST_RESET.
    JustReset {
        /// The reset time that passed (`Reset at 09:00 · waiting for the first reading`).
        previous: Timestamp,
    },
    /// `starts_on_use` window, or `L` unknown, whose `R` passed: `u = 0`, `R` none, shown like
    /// IDLE until a reading reports a new `resets_at`.
    IdleAfterReset {
        /// The reset time that passed.
        previous: Timestamp,
    },
}

/// The window as it should be shown at `now` (R§3.9).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectiveWindow {
    /// `u` to show: the reading's, or 0 after a reset.
    pub used: f64,
    /// `R` to show: the reading's, `R'` after a fixed-schedule reset, none after a
    /// `starts_on_use`/unknown-`L` reset.
    pub resets_at: Option<Timestamp>,
    /// `L` for the instance that ends at `resets_at` (`None` = unknown).
    pub window: Option<SignedDuration>,
    /// Which case applied.
    pub view: ResetView,
}

/// Applies R§3.9 to a meter reading at `now`. A stale meter whose `R ≤ now` is drawn as reset
/// but keeps the stale styling (R§3.8); the caller decides the styling.
pub fn effective_window(meter: &Meter, now: Timestamp) -> EffectiveWindow {
    let live = EffectiveWindow {
        used: meter.used,
        resets_at: meter.resets_at,
        window: match meter.resets_at {
            Some(r) => meter.window.length_ending_at(r),
            None => length_without_reset(meter.window),
        },
        view: ResetView::Live,
    };
    let Some(reset) = meter.resets_at else {
        return live;
    };
    if now < reset {
        return live;
    }
    let fixed_schedule = !meter.starts_on_use && meter.window != WindowLen::Unknown;
    if fixed_schedule && let Some((last, next)) = advance(meter.window, reset, now) {
        return EffectiveWindow {
            used: 0.0,
            resets_at: Some(next),
            window: meter.window.length_ending_at(next),
            view: ResetView::JustReset { previous: last },
        };
    }
    EffectiveWindow {
        used: 0.0,
        resets_at: None,
        window: length_without_reset(meter.window),
        view: ResetView::IdleAfterReset { previous: reset },
    }
}

/// When to refresh a service after a reset at `resets_at` (R§3.9): `R + jitter`, where the
/// caller draws `jitter` uniformly from `RESET_REFRESH_JITTER` (30–90 s) so this stays pure;
/// moved later to `last_fetch + min_interval` when the last fetch is more recent than that
/// (the 5-minute minimum of undocumented endpoints). Back-off is applied by
/// [`crate::refresh::next_due`].
pub fn reset_refresh_at(
    resets_at: Timestamp,
    jitter: SignedDuration,
    last_fetch: Option<Timestamp>,
    min_interval: SignedDuration,
) -> Timestamp {
    let at = add(resets_at, jitter);
    match last_fetch {
        Some(last) => at.max(add(last, min_interval)),
        None => at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MeterId, MeterNames};

    fn ts(mins: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + mins * 60).expect("valid")
    }

    fn meter(window: WindowLen, starts_on_use: bool, resets_at: Option<Timestamp>) -> Meter {
        Meter {
            id: MeterId::new("m"),
            names: MeterNames::new("M", "m", "M"),
            order: 0,
            used: 0.5,
            limited: false,
            resets_at,
            window,
            starts_on_use,
        }
    }

    #[test]
    fn live_before_the_reset() {
        let m = meter(
            WindowLen::Fixed(SignedDuration::from_hours(5)),
            false,
            Some(ts(60)),
        );
        let w = effective_window(&m, ts(59));
        assert_eq!(w.view, ResetView::Live);
        assert_eq!((w.used, w.resets_at), (0.5, Some(ts(60))));
        assert_eq!(w.window, Some(SignedDuration::from_hours(5)));
    }

    #[test]
    fn fixed_schedule_skips_every_passed_window() {
        let m = meter(
            WindowLen::Fixed(SignedDuration::from_hours(5)),
            false,
            Some(ts(0)),
        );
        let w = effective_window(&m, ts(0));
        assert_eq!(w.view, ResetView::JustReset { previous: ts(0) });
        assert_eq!(w.resets_at, Some(ts(300)));
        // Three windows later (a laptop that slept): the latest passed reset and the next one.
        let w = effective_window(&m, ts(3 * 300 + 10));
        assert_eq!(w.view, ResetView::JustReset { previous: ts(900) });
        assert_eq!(w.resets_at, Some(ts(1200)));
        assert_eq!(w.used, 0.0);
    }

    #[test]
    fn starts_on_use_and_unknown_length_go_idle() {
        let five = WindowLen::Fixed(SignedDuration::from_hours(5));
        let w = effective_window(&meter(five, true, Some(ts(10))), ts(11));
        assert_eq!(w.view, ResetView::IdleAfterReset { previous: ts(10) });
        assert_eq!((w.used, w.resets_at), (0.0, None));
        let w = effective_window(&meter(WindowLen::Unknown, false, Some(ts(10))), ts(11));
        assert_eq!(w.view, ResetView::IdleAfterReset { previous: ts(10) });
        assert_eq!(w.window, None);
        // No reset time at all: live, nothing to reset.
        let w = effective_window(&meter(five, true, None), ts(11));
        assert_eq!(w.view, ResetView::Live);
    }

    #[test]
    fn calendar_month_resets_to_the_next_month_start() {
        let nov1 = "2026-11-01T00:00:00Z".parse::<Timestamp>().expect("ts");
        let m = meter(WindowLen::CalendarMonth, false, Some(nov1));
        let w = effective_window(&m, add(nov1, SignedDuration::from_hours(1)));
        let dec1 = "2026-12-01T00:00:00Z".parse::<Timestamp>().expect("ts");
        assert_eq!(w.resets_at, Some(dec1));
        assert_eq!(w.window, Some(SignedDuration::from_hours(30 * 24)));
    }

    #[test]
    fn reset_refresh_waits_for_the_minimum_interval() {
        let min = SignedDuration::from_mins(5);
        let jitter = SignedDuration::from_secs(45);
        assert_eq!(
            reset_refresh_at(ts(100), jitter, Some(ts(90)), min),
            add(ts(100), jitter)
        );
        assert_eq!(
            reset_refresh_at(ts(100), jitter, Some(ts(99)), min),
            ts(104)
        );
        assert_eq!(
            reset_refresh_at(ts(100), jitter, None, min),
            add(ts(100), jitter)
        );
    }
}
