//! The forecast: "at the rate you have been using it recently, when is it empty?" (R§3.5).

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::history::Sample;
use crate::consts::{
    FC_FRESH_GUARD, FC_MIN_SPAN, S_DIVISOR, S_MAX, S_MIN, S_UNKNOWN_WINDOW, SAME_WINDOW_TOLERANCE,
};
use crate::grammar::time::{floor_min, round_min};
use crate::util::{duration_from_hours, hours_f64, sane_share, sub};

/// The forecast of a window meter, anchored at the latest reading `t_n` (R§3.5).
///
/// Computed per reading; between readings only `N` ([`runway_minutes`]) changes, re-evaluated on
/// every minute tick (R§12).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    /// `t_n`: the latest reading.
    pub anchor: Timestamp,
    /// `u_n`: used share at `t_n`.
    pub latest_used: f64,
    /// `S`: the trailing span ([`trailing_span`]).
    pub trailing: SignedDuration,
    /// `span = t_n − t_a`.
    pub span: SignedDuration,
    /// `rate = max(0, (u_n − u_a) / span)`, share per hour. A decrease inside a window instance
    /// (provider correction) gives 0.
    pub rate_per_hour: f64,
    /// `p = u_n + rate × (R − t_n)`: projected share used at the reset.
    pub projected: f64,
    /// `t_out = (rate > 0 && u_n < 1) ? t_n + (1 − u_n) / rate : none`.
    pub t_out: Option<Timestamp>,
    /// `valid = R ≠ none && span ≥ FC_MIN_SPAN && t_n − start ≥ FC_FRESH_GUARD`. Not valid → the
    /// meter can be OK or critical by level, never warning or critical by runway.
    pub valid: bool,
    /// `runs_out = valid && t_out ≠ none && t_out < R`.
    pub runs_out: bool,
}

/// `S = clamp(L / 10, 30 min, 24 h)`; `L` unknown → 1 h (R§3.5, R§3.11).
/// 5 h → 30 min, 7 d → 16.8 h, a month → 24 h.
pub fn trailing_span(window: Option<SignedDuration>) -> SignedDuration {
    match window {
        Some(len) => (len / S_DIVISOR).clamp(S_MIN, S_MAX),
        None => S_UNKNOWN_WINDOW,
    }
}

/// Two readings belong to the same window instance when their `R` differ by at most 2 minutes
/// (R§3.1). Two `None`s are the same instance (a window without reset times); `None` and `Some`
/// are not.
pub fn same_window_instance(a: Option<Timestamp>, b: Option<Timestamp>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.duration_since(b).abs() <= SAME_WINDOW_TOLERANCE,
        _ => false,
    }
}

/// The window instance (its `R`) a reading belongs to, given the instance of the reading before
/// it (R§3.1): the reading's own `R`; for a reading without a reset time, the previous reading's
/// instance when it was taken before that instance's reset, else none.
///
/// A source that reports no reset times (Claude Desktop's history) says nothing about the
/// instance, so its readings inside a known instance belong to that instance instead of starting
/// a new one: losing the reset time is not a window reset (R§3.7, R§3.9). **Not from the spec**:
/// R§3.1 defines instances by `R` only.
pub fn instance_of(previous: Option<Timestamp>, sample: &Sample) -> Option<Timestamp> {
    match sample.resets_at {
        Some(r) => Some(r),
        None => previous.filter(|&r| sample.t < r),
    }
}

/// The instance of the last of `samples` (ascending), folding [`instance_of`] over them: the
/// last reading's `R`, else the `R` of the newest reading that has one, while the last reading
/// precedes it.
pub fn latest_instance(samples: &[Sample]) -> Option<Timestamp> {
    let last = samples.last()?;
    if last.resets_at.is_some() {
        return last.resets_at;
    }
    samples
        .iter()
        .rev()
        .find_map(|s| s.resets_at)
        .filter(|&r| last.t < r)
}

/// The longest suffix of `samples` (ascending) whose readings are in the same window instance as
/// the last one (R§3.1), instances per [`instance_of`]: readings without a reset time inside a
/// known instance belong to it.
pub fn current_instance(samples: &[Sample]) -> &[Sample] {
    let target = latest_instance(samples);
    let mut start = 0;
    let mut previous = None;
    for (i, s) in samples.iter().enumerate() {
        previous = instance_of(previous, s);
        if !same_window_instance(previous, target) {
            start = i + 1;
        }
    }
    &samples[start..]
}

/// The forecast of R§3.5 from the readings of one window instance (ascending, latest last;
/// see [`current_instance`]):
///
/// ```text
/// S     = clamp(L / 10, 30 min, 24 h)                 // L unknown: 1 h
/// start = R − L                                       // L unknown: t of the first reading
/// a     = latest reading with t ≤ t_n − S, else the oldest reading of the instance
/// span  = t_n − t_a
/// valid = R ≠ none && span ≥ FC_MIN_SPAN && t_n − start ≥ FC_FRESH_GUARD
/// rate  = max(0, (u_n − u_a) / span)                  // share per hour
/// p     = u_n + rate × (R − t_n)
/// t_out = (rate > 0 && u_n < 1) ? t_n + (1 − u_n) / rate : none
/// ```
///
/// Returns `None` when `instance` is empty or `R` is none (no reset time → no forecast, R§3.2).
/// `window` is `L` for this instance (`None` = unknown).
pub fn forecast(
    instance: &[Sample],
    resets_at: Option<Timestamp>,
    window: Option<SignedDuration>,
) -> Option<Forecast> {
    let reset = resets_at?;
    let first = instance.first()?;
    let latest = instance.last()?;
    let trailing = trailing_span(window);
    let start = match window {
        Some(len) => sub(reset, len),
        None => first.t,
    };
    let cutoff = sub(latest.t, trailing);
    let anchor = instance
        .iter()
        .rev()
        .find(|s| s.t <= cutoff)
        .unwrap_or(first);
    let span = latest.t.duration_since(anchor.t);
    let valid = span >= FC_MIN_SPAN && latest.t.duration_since(start) >= FC_FRESH_GUARD;
    let u_n = sane_share(latest.value);
    let span_h = hours_f64(span);
    let rate_per_hour = if span_h > 0.0 {
        ((u_n - sane_share(anchor.value)) / span_h).max(0.0)
    } else {
        0.0
    };
    // A reading taken after its own reset time projects nothing more.
    let to_reset_h = hours_f64(reset.duration_since(latest.t)).max(0.0);
    let projected = u_n + rate_per_hour * to_reset_h;
    let t_out = if rate_per_hour > 0.0 && u_n < 1.0 {
        // A rate so small that the run-out is beyond any representable time never runs out.
        duration_from_hours((1.0 - u_n) / rate_per_hour).and_then(|d| latest.t.checked_add(d).ok())
    } else {
        None
    };
    let runs_out = valid && t_out.is_some_and(|t| t < reset);
    Some(Forecast {
        anchor: latest.t,
        latest_used: u_n,
        trailing,
        span,
        rate_per_hour,
        projected,
        t_out,
        valid,
        runs_out,
    })
}

/// `N`: whole minutes from `floor_min(now)` to `round_min(t_out)`, at least 1 — the runway as
/// shown (R§3.5, R§5.0). Runway thresholds compare `N`, so `30 min left` is always red and
/// `31 min left` never is.
pub fn runway_minutes(now: Timestamp, t_out: Timestamp) -> i64 {
    round_min(t_out)
        .duration_since(floor_min(now))
        .as_mins()
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::add;

    fn ts(mins: i64) -> Timestamp {
        add(Timestamp::UNIX_EPOCH, SignedDuration::from_mins(mins))
    }

    #[test]
    fn trailing_span_clamps() {
        assert_eq!(trailing_span(Some(SignedDuration::from_hours(5))), S_MIN);
        assert_eq!(
            trailing_span(Some(SignedDuration::from_hours(7 * 24))),
            SignedDuration::from_mins(1008)
        );
        assert_eq!(
            trailing_span(Some(SignedDuration::from_hours(31 * 24))),
            S_MAX
        );
        assert_eq!(trailing_span(None), SignedDuration::from_hours(1));
    }

    #[test]
    fn window_instances_tolerate_two_minutes_of_jitter() {
        let r = ts(1000);
        assert!(same_window_instance(
            Some(r),
            Some(add(r, SignedDuration::from_secs(119)))
        ));
        assert!(same_window_instance(
            Some(r),
            Some(sub(r, SignedDuration::from_mins(2)))
        ));
        assert!(!same_window_instance(
            Some(r),
            Some(add(r, SignedDuration::from_mins(3)))
        ));
        assert!(same_window_instance(None, None));
        assert!(!same_window_instance(None, Some(r)));
        let samples = [
            Sample::window(ts(0), 0.9, Some(ts(300))),
            Sample::window(ts(10), 0.95, Some(ts(300))),
            Sample::window(ts(310), 0.0, Some(ts(600))),
            Sample::window(ts(320), 0.1, Some(ts(601))),
        ];
        assert_eq!(current_instance(&samples).len(), 2);
        assert!(current_instance(&[]).is_empty());
    }

    #[test]
    fn readings_without_a_reset_time_stay_in_their_instance() {
        let r = ts(300);
        // OAuth readings with R, desktop-history readings without, inside one window.
        let samples = [
            Sample::window(ts(0), 0.1, Some(ts(290))),
            Sample::window(ts(290), 0.0, None),
            Sample::window(ts(300), 0.2, Some(ts(600))),
            Sample::window(ts(310), 0.25, None),
            Sample::window(ts(320), 0.3, Some(ts(601))),
            Sample::window(ts(330), 0.35, None),
        ];
        assert_eq!(latest_instance(&samples), Some(ts(601)));
        assert_eq!(current_instance(&samples).len(), 4);
        assert_eq!(
            instance_of(Some(r), &Sample::window(ts(299), 0.5, None)),
            Some(r)
        );
        // Taken at or after the reset: the instance is over, the reading belongs to none.
        assert_eq!(
            instance_of(Some(r), &Sample::window(ts(300), 0.0, None)),
            None
        );
        let after = [
            Sample::window(ts(0), 0.9, Some(r)),
            Sample::window(ts(100), 0.95, None),
            Sample::window(ts(305), 0.1, None),
            Sample::window(ts(310), 0.2, None),
        ];
        assert_eq!(latest_instance(&after), None);
        assert_eq!(current_instance(&after).len(), 2);
        // Only readings without reset times: one instance.
        let none = [
            Sample::window(ts(0), 0.1, None),
            Sample::window(ts(10), 0.2, None),
        ];
        assert_eq!(current_instance(&none).len(), 2);
    }

    #[test]
    fn no_forecast_without_reset_or_readings() {
        let s = [Sample::window(ts(0), 0.1, None)];
        assert!(forecast(&s, None, Some(SignedDuration::from_hours(5))).is_none());
        assert!(forecast(&[], Some(ts(300)), Some(SignedDuration::from_hours(5))).is_none());
    }

    #[test]
    fn single_reading_and_corrections_give_no_rate() {
        let r = ts(300);
        let h5 = Some(SignedDuration::from_hours(5));
        let one = forecast(&[Sample::window(ts(100), 0.5, Some(r))], Some(r), h5).expect("f");
        assert!(!one.valid);
        assert_eq!(one.rate_per_hour, 0.0);
        assert_eq!(one.t_out, None);
        assert_eq!(one.projected, 0.5);
        // A provider correction downwards is rate 0, not a negative rate.
        let down = [
            Sample::window(ts(100), 0.6, Some(r)),
            Sample::window(ts(140), 0.5, Some(r)),
        ];
        let f = forecast(&down, Some(r), h5).expect("f");
        assert!(f.valid);
        assert_eq!(f.rate_per_hour, 0.0);
        assert!(!f.runs_out);
    }

    #[test]
    fn used_up_has_no_run_out_time() {
        let r = ts(300);
        let s = [
            Sample::window(ts(100), 0.8, Some(r)),
            Sample::window(ts(140), 1.2, Some(r)),
        ];
        let f = forecast(&s, Some(r), Some(SignedDuration::from_hours(5))).expect("f");
        assert!(f.rate_per_hour > 0.0);
        assert_eq!(f.t_out, None);
        assert!(!f.runs_out);
        assert!(f.projected > 1.0);
    }

    #[test]
    fn runway_is_at_least_one_minute() {
        let now = ts(100);
        assert_eq!(runway_minutes(now, ts(90)), 1);
        assert_eq!(runway_minutes(now, ts(112)), 12);
    }
}
