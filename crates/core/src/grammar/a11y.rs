//! UI Automation sentences (R§11) and the tray tooltip (R§9.3).
//!
//! Row sentence grammar: `<Service> <meter name>: <verdict>, <facts>.` — verdict first; full
//! times with AM/PM where the locale uses it ([`super::time::spoken_time`]). `{meter}` here is
//! the meter's **spoken name**: its `name` with ` · ` read as `, ` and the first letter
//! lower-cased unless it is a digit — `5-hour session`, `weekly, all models`, `weekly, Fable`
//! (R§11's examples; a screen reader would read `·` aloud). Percentages are spoken as
//! `{n} percent` with a bare integer.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use super::TextContext;
use crate::stats::{Assessment, Evaluation, ServiceEval};

/// The sentence of one readout row / flyout meter row (R§11 examples):
///
/// - `Claude 5-hour session: on track, 42 percent used, resets at 2:56 PM.`
/// - `Claude 5-hour session: warning, runs out at about 3:20 PM, before the reset at 4:30 PM; 78 percent used.`
/// - `Claude 5-hour session: critical, runs out in about 12 minutes; 94 percent used; resets at 4:30 PM.`
/// - `Claude 5-hour session: critical, 92 percent used, lasts until the reset at 4:30 PM.`
/// - `Claude weekly, all models: critical, runs out in about 6 hours; 91 percent used; resets Monday 9:00 AM.`
/// - `Claude 5-hour session: empty, back at 4:30 PM.`
/// - `Claude 5-hour session: not started, 0 percent used, starts with your next message.`
/// - `Meshy: 1,240 credits left, about 9 days at the current rate.`
///
/// A stale meter is described by its service's sentence ([`service_sentence`]).
pub fn row_sentence(a: &Assessment, service: &ServiceEval, cx: &TextContext<'_>) -> String {
    todo!("M1: R§11 row sentence")
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
    todo!("M1: R§11 stale service sentence")
}

/// The readout root's name: `Lastdrop. ` + the row sentences (R§11, ControlType Button).
pub fn root_name(sentences: &[String], cx: &TextContext<'_>) -> String {
    todo!("M1: R§11 root name")
}

/// The tray icon's `szTip`, ≤ 127 characters (`TRAY_TIP_MAX_CHARS`), also its accessible name
/// (R§9.3): `Lastdrop: Claude weekly 61% used, resets Mon 09:00` ·
/// `Lastdrop: Claude 5-hour runs out in 12 min` · `Lastdrop: Claude offline, last reading 47 min ago`.
/// Describes the winner (R§3.10).
pub fn tray_tooltip(eval: &Evaluation, cx: &TextContext<'_>) -> String {
    todo!("M1: R§9.3")
}
