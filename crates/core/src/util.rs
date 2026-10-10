//! Small numeric and time helpers shared by the statistics, the grammar and the pixel algorithms.
//! Private to the crate: the public API speaks in spec terms, these are the arithmetic under it.

use jiff::{SignedDuration, Timestamp};

/// `round` of the spec: round half up (R§0 "round is round-half-up everywhere in this spec").
/// `x − floor(x)` is exact for the magnitudes used here, so `0.49999999999999994` stays below
/// one half (unlike `floor(x + 0.5)`).
pub(crate) fn round_half_up(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// [`round_half_up`] to an `i32`, saturating; NaN gives 0.
pub(crate) fn round_i32(x: f64) -> i32 {
    // `as` saturates at the bounds and maps NaN to 0.
    round_half_up(x) as i32
}

/// A duration in hours as a float.
pub(crate) fn hours_f64(d: SignedDuration) -> f64 {
    d.as_secs_f64() / 3600.0
}

/// A float number of hours as a duration; `None` when it is not finite or out of range.
pub(crate) fn duration_from_hours(h: f64) -> Option<SignedDuration> {
    SignedDuration::try_from_secs_f64(h * 3600.0).ok()
}

/// `t + d`, saturating at the ends of the representable range instead of panicking (provider data
/// can be absurd; a statistic must never crash the app).
pub(crate) fn add(t: Timestamp, d: SignedDuration) -> Timestamp {
    match t.checked_add(d) {
        Ok(v) => v,
        Err(_) if d.is_negative() => Timestamp::MIN,
        Err(_) => Timestamp::MAX,
    }
}

/// `t − d`, saturating.
pub(crate) fn sub(t: Timestamp, d: SignedDuration) -> Timestamp {
    match t.checked_sub(d) {
        Ok(v) => v,
        Err(_) if d.is_negative() => Timestamp::MAX,
        Err(_) => Timestamp::MIN,
    }
}

/// A share that is safe to compute with: NaN becomes 0, negative values are clamped at 0 (R§3.1
/// "never negative").
pub(crate) fn sane_share(u: f64) -> f64 {
    if u.is_nan() { 0.0 } else { u.max(0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_half_up() {
        assert_eq!(round_half_up(2.5), 3.0);
        assert_eq!(round_half_up(2.4999), 2.0);
        assert_eq!(round_half_up(-2.5), -2.0);
        assert_eq!(round_half_up(0.499_999_999_999_999_94), 0.0);
        assert_eq!(round_i32(f64::NAN), 0);
        assert_eq!(round_i32(1e300), i32::MAX);
    }

    #[test]
    fn saturating_time_arithmetic() {
        assert_eq!(
            add(Timestamp::MAX, SignedDuration::from_hours(1)),
            Timestamp::MAX
        );
        assert_eq!(
            sub(Timestamp::MIN, SignedDuration::from_hours(1)),
            Timestamp::MIN
        );
        assert_eq!(
            add(Timestamp::UNIX_EPOCH, SignedDuration::from_secs(60)),
            Timestamp::from_second(60).expect("valid")
        );
    }

    #[test]
    fn shares_are_sanitized() {
        assert_eq!(sane_share(f64::NAN), 0.0);
        assert_eq!(sane_share(-0.2), 0.0);
        assert_eq!(sane_share(1.3), 1.3);
        assert!(duration_from_hours(f64::INFINITY).is_none());
        assert_eq!(hours_f64(SignedDuration::from_mins(90)), 1.5);
    }
}
