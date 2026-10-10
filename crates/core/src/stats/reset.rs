//! Window reset handling when `now ≥ R` and no newer reading has arrived (R§3.9).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::model::Meter;

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
    todo!("M1: R§3.9")
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
    todo!("M1: R§3.9 reset refresh")
}
