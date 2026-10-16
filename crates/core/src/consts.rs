//! Design constants in one module (R§0: "keep them in one module so they can be tuned without
//! touching logic").
//!
//! The first block is R§3.11 verbatim. [`layout`] holds the logical-pixel numbers of R§6, R§8,
//! R§9 and F§1–F§6. Constants marked **not from the spec** are Lastdrop's own choices where the
//! spec names a behaviour but no number; tune them freely.

use std::time::Duration;

use jiff::SignedDuration;

// ---- R§3.11 -----------------------------------------------------------------------------------

/// `TOL_PACE` = 0.03 (R§3.4, R§6.6): the pace tolerance as a share. `under` is `u < e − TOL`,
/// `ahead` is `u > e + TOL`.
pub const TOL_PACE: f64 = 0.03;

/// `FC_MIN_SPAN` = 15 min (R§3.5): a forecast needs two readings at least this far apart.
pub const FC_MIN_SPAN: SignedDuration = SignedDuration::from_mins(15);

/// `FC_FRESH_GUARD` = 15 min after the window start (R§3.5, R§3.7): no forecast before it.
pub const FC_FRESH_GUARD: SignedDuration = SignedDuration::from_mins(15);

/// Lower clamp of the trailing span `S = clamp(L / 10, 30 min, 24 h)` (R§3.5).
pub const S_MIN: SignedDuration = SignedDuration::from_mins(30);

/// Upper clamp of the trailing span `S = clamp(L / 10, 30 min, 24 h)` (R§3.5).
pub const S_MAX: SignedDuration = SignedDuration::from_hours(24);

/// Divisor in `S = L / 10` (R§3.5).
pub const S_DIVISOR: i32 = 10;

/// `S` when the window length `L` is unknown: 1 h (R§3.5).
pub const S_UNKNOWN_WINDOW: SignedDuration = SignedDuration::from_hours(1);

/// Two readings belong to the same window instance when their `resets_at` differ by at most
/// 2 minutes (R§3.1; providers jitter `resets_at` by seconds).
pub const SAME_WINDOW_TOLERANCE: SignedDuration = SignedDuration::from_mins(2);

/// `CRIT_LEVEL_PCT` = 90 (R§3.7): CRITICAL by level when `pct ≥ 90` (compares the shown `pct`).
pub const CRIT_LEVEL_PCT: u8 = 90;

/// Exit of critical-by-level: leaves when `pct ≤ 87` (R§3.7).
pub const CRIT_LEVEL_EXIT_PCT: u8 = 87;

/// `CRIT_RUNWAY` = 30 (R§3.7): CRITICAL by runway when `runs_out && N ≤ 30` (shown minutes).
pub const CRIT_RUNWAY_MIN: i64 = 30;

/// Exit of critical-by-runway: leaves when `N ≥ 36` or `!runs_out` (R§3.7).
pub const CRIT_RUNWAY_EXIT_MIN: i64 = 36;

/// `WARN_EXIT_P` = 0.97 (R§3.7): WARNING leaves when `p ≤ 0.97` or the forecast is not valid.
pub const WARN_EXIT_P: f64 = 0.97;

/// `COUNTDOWN_MAX_MIN` = 59 (R§5.3): a run-out up to 59 shown minutes away counts down at any
/// severity; CRITICAL counts down at any `N`.
pub const COUNTDOWN_MAX_MIN: i64 = 59;

/// `COUNTDOWN_HOURS_BELOW` = 48 h (R§5.3): countdowns below it are in hours, from it in days.
pub const COUNTDOWN_HOURS_BELOW: SignedDuration = SignedDuration::from_hours(48);

/// `TIME_OF_DAY_MAX` = 12 h (R§5.2): time points less than 12 h away are shown as a time of day.
pub const TIME_OF_DAY_MAX: SignedDuration = SignedDuration::from_hours(12);

/// `STALE_MIN` = 15 min (R§3.8): `stale_after = max(15 min, 2 × poll_interval)`.
pub const STALE_MIN: SignedDuration = SignedDuration::from_mins(15);

/// Factor in `stale_after = max(15 min, 2 × poll_interval)` (R§3.8).
pub const STALE_INTERVAL_FACTOR: i32 = 2;

/// `stale_after` in manual refresh mode: 60 min (R§3.8).
pub const STALE_MANUAL: SignedDuration = SignedDuration::from_mins(60);

/// `RESET_REFRESH_JITTER` lower bound: 30 s (R§3.9).
pub const RESET_REFRESH_JITTER_MIN: SignedDuration = SignedDuration::from_secs(30);

/// `RESET_REFRESH_JITTER` upper bound: 90 s (R§3.9).
pub const RESET_REFRESH_JITTER_MAX: SignedDuration = SignedDuration::from_secs(90);

/// `BAL_WARN_DAYS` = 7 (R§3.7): a balance is WARNING when `days_left < 7`.
pub const BAL_WARN_DAYS: f64 = 7.0;

/// Balance WARNING leaves at `days_left ≥ 8` (R§3.7).
pub const BAL_WARN_EXIT_DAYS: f64 = 8.0;

/// `BAL_CRIT_DAYS` = 2 (R§3.7): a balance is CRITICAL when `days_left < 2`.
pub const BAL_CRIT_DAYS: f64 = 2.0;

/// Balance CRITICAL leaves at `days_left ≥ 2.5` (R§3.7).
pub const BAL_CRIT_EXIT_DAYS: f64 = 2.5;

/// `BAL_SPAN` = 7 d (R§3.6): burn = sum of decreases over the last 7 days / 7.
pub const BAL_SPAN: SignedDuration = SignedDuration::from_hours(7 * 24);

/// `BAL_MIN_HISTORY` = 48 h (R§3.6): a balance forecast needs history covering at least 48 h.
pub const BAL_MIN_HISTORY: SignedDuration = SignedDuration::from_hours(48);

/// Without a detected top-up, `ref` is the highest balance seen in the last 30 days (R§3.6).
pub const BAL_REF_LOOKBACK: SignedDuration = SignedDuration::from_hours(30 * 24);

/// A top-up is an increase of at least 1 % of the balance (R§3.6).
pub const BAL_TOPUP_MIN_SHARE: f64 = 0.01;

/// `AUTO_SWAP_MARGIN` = 0.05 (R§3.10): a same-class challenger replaces a shown row only with
/// `q ≥ q_shown + 0.05`.
pub const AUTO_SWAP_MARGIN: f64 = 0.05;

/// `AUTO_SWAP_INTERVAL` = 10 min (R§3.10): same-class swaps happen at most once per interval.
pub const AUTO_SWAP_INTERVAL: SignedDuration = SignedDuration::from_mins(10);

/// Auto rows of the two-row readout, N = 2 (R§3.10; the compact form shows 1 of them, R§4.2).
/// Also the number of places the "too few candidates" rule fills with OK balance meters.
pub const AUTO_ROWS: usize = 2;

/// R§5.5: a balance duration is in hours while `days_left` is below 2 days.
pub const BAL_DURATION_HOURS_BELOW_DAYS: f64 = 2.0;

/// R§5.5: in days while `days_left` is below 14 days.
pub const BAL_DURATION_DAYS_BELOW_DAYS: f64 = 14.0;

/// R§5.5: in weeks while `days_left` is below 63 days; from it `9w+` (F§4.5 `More than 8 weeks
/// left`).
pub const BAL_DURATION_WEEKS_BELOW_DAYS: f64 = 63.0;

/// `HOVER_DELAY` default, 400 ms (R§10). The live value is `SPI_GETMOUSEHOVERTIME`.
pub const HOVER_DELAY_DEFAULT: Duration = Duration::from_millis(400);

// ---- Other spec numbers -----------------------------------------------------------------------

/// The hover card hides 100 ms after the pointer leaves (R§8.3).
pub const HOVER_LEAVE_GRACE: Duration = Duration::from_millis(100);

/// Undocumented endpoints are polled at most every 5 minutes per credential (ARCHITECTURE
/// "Rules for every provider", R§3.9).
pub const UNDOCUMENTED_MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Ignore an "open" of the flyout within 300 ms of a hide caused by deactivation (F§1.4).
pub const FLYOUT_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(300);

/// The minute tick fires 250 ms after the local minute boundary (R§12).
pub const MINUTE_TICK_OFFSET: Duration = Duration::from_millis(250);

/// History is thinned to one sample per meter per 5 minutes (ARCHITECTURE "Persistence").
pub const HISTORY_THIN_INTERVAL: SignedDuration = SignedDuration::from_mins(5);

/// History is kept for 90 days (ARCHITECTURE "Persistence").
pub const HISTORY_RETENTION: SignedDuration = SignedDuration::from_hours(90 * 24);

/// Days in the flyout sparkline (F§6).
pub const SPARK_DAYS: usize = 14;

/// Sparkline scale floor for window services: 10 percentage points (F§6).
pub const SPARK_WINDOW_FLOOR_PP: f64 = 10.0;

/// Sparkline scale floor for balances: 5 % of `ref` (F§6).
pub const SPARK_BALANCE_FLOOR_SHARE: f64 = 0.05;

/// "Also tracked" lists at most 4 meters, then `+N more` (R§8.2).
pub const ALSO_TRACKED_MAX: usize = 4;

/// `szTip` holds at most 127 characters (R§9.3).
pub const TRAY_TIP_MAX_CHARS: usize = 127;

/// Shown percentage that the hover card's "on course for {p}%" clamps to (R§8.2).
pub const CARD_FORECAST_MAX_PCT: u8 = 99;

// ---- Not from the spec ------------------------------------------------------------------------

/// First back-off after a failed fetch (**not from the spec**; R§3.8/ARCHITECTURE only say
/// "back off on 429/403"). Doubles per consecutive failure up to [`BACKOFF_MAX`]; a provider's
/// `Retry-After` always wins when it is longer.
pub const BACKOFF_INITIAL: Duration = Duration::from_secs(5 * 60);

/// Longest back-off between attempts (**not from the spec**).
pub const BACKOFF_MAX: Duration = Duration::from_secs(60 * 60);

/// Adaptive refresh while the flyout is open: poll every minute, never below the provider's
/// `min_interval` (**not from the spec**; ARCHITECTURE "faster while the flyout is open").
pub const ADAPTIVE_OPEN_INTERVAL: Duration = Duration::from_secs(60);

/// Adaptive refresh while the user is idle: the interval is multiplied by this factor
/// (**not from the spec**; ARCHITECTURE "slower when idle").
pub const ADAPTIVE_IDLE_FACTOR: u32 = 3;

/// Upper bound of the adaptive idle interval (**not from the spec**; matches the 30-minute
/// maximum of the interval setting in ROADMAP "Settings").
pub const ADAPTIVE_IDLE_MAX: Duration = Duration::from_secs(30 * 60);

/// The user counts as idle for the adaptive refresh after this long without keyboard or mouse
/// input in the session; a locked session or a display turned off for inactivity has none
/// (**not from the spec**; ARCHITECTURE "slower when idle").
pub const ADAPTIVE_IDLE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Default refresh interval in minutes (**not from the spec**; R§3.8's examples use 5 min and
/// undocumented endpoints allow no less).
pub const DEFAULT_REFRESH_MINUTES: u16 = 5;

/// Bounds of the refresh interval setting in minutes (ROADMAP "Settings": "1–30 min").
pub const REFRESH_MINUTES_RANGE: (u16, u16) = (1, 30);

// ---- Notifications (N§) ---------------------------------------------------------------------

/// The default notification levels (N§2; ROADMAP "Notifications at thresholds (for example 80 %
/// and 95 %)").
pub const NOTIFY_DEFAULT_LEVELS: [u8; 2] = [80, 95];

/// The level a used-up window is remembered at, above every user level (N§1 USED_UP).
pub const NOTIFY_USED_UP_LEVEL: u8 = 100;

/// Other notices listed under the first one's text before `+{n} more` (N§3).
pub const NOTIFY_MAX_LISTED: usize = 2;

/// A window's announced levels are forgotten this long after its reset time, when no evaluation
/// saw the reset (the meter went away, or stayed stale; N§1).
pub const NOTIFY_FORGET_AFTER: SignedDuration = SignedDuration::from_hours(24);

/// Layout numbers in logical px at 100 % (R§0). Device px = `round(x × s)` per constant.
pub mod layout {
    // -- Readout (R§6)
    /// Readout slot height, logical px (R§6.1).
    pub const SLOT_H: f32 = 40.0;
    /// Readout row height (R§6.1); also the pace bar's `rowH` in the readout (R§6.6).
    pub const ROW_H: f32 = 16.0;
    /// Horizontal padding inside the slot, both sides (R§6.1).
    pub const SLOT_PAD_X: f32 = 6.0;
    /// Gap between the slot's right edge and the overflow chevron or notification area (R§6.1).
    pub const SLOT_RIGHT_GAP: f32 = 4.0;
    /// Service glyph box (R§6.2).
    pub const SERVICE_GLYPH: f32 = 12.0;
    /// Gap after the service glyph (R§6.2).
    pub const SERVICE_GLYPH_GAP: f32 = 4.0;
    /// Gap between label, bar and right cell (R§6.2).
    pub const COLUMN_GAP: f32 = 6.0;
    /// Readout bar length `W` (R§6.2, R§6.6).
    pub const BAR_W: f32 = 40.0;
    /// VALUE box width inside the right cell, value right-aligned (R§5.7).
    pub const VALUE_BOX_W: f32 = 24.0;
    /// x of the time/status sub-column inside the right cell (R§5.7).
    pub const TIME_X: f32 = 30.0;
    /// Reset glyph cell (U+E777 at 11 px) and its gap (R§6.5: "cell 11 + 1 gap").
    pub const RESET_GLYPH: f32 = 11.0;
    /// Gap after the reset glyph (R§6.5).
    pub const RESET_GLYPH_GAP: f32 = 1.0;
    /// The gap after `≈` in the balance term of `RIGHT_W` (R§6.3: `width("≈") + 1`).
    pub const BALANCE_APPROX_GAP: f32 = 1.0;
    /// Severity glyph cell (12 px) in say-text (R§6.5: "cell 12 + 2 gap").
    pub const SEVERITY_GLYPH: f32 = 12.0;
    /// Gap after a severity glyph in say-text (R§6.5).
    pub const SEVERITY_GLYPH_GAP: f32 = 2.0;
    /// Compact cell: glyph 12 + 2 before semibold text (R§6.3 `COMPACT_W`).
    pub const COMPACT_GLYPH_ADVANCE: f32 = 14.0;
    /// Hysteresis for going up a readout form: `room ≥ width(form) + 8` (R§4.2).
    pub const FORM_UP_HYSTERESIS: f32 = 8.0;
    /// `room` subtracts 4 px before the chevron (R§4.2).
    pub const ROOM_CHEVRON_GAP: f32 = 4.0;
    /// `room` keeps 8 px after the rightmost app button (R§4.2).
    pub const ROOM_APP_GAP: f32 = 8.0;
    /// Taskbars shorter than 40 px get the compact form (R§4.2).
    pub const MIN_TWO_ROW_TASKBAR_H: f32 = 40.0;
    /// Hover/pressed plate corner radius (R§6.8).
    pub const PLATE_RADIUS: f32 = 4.0;
    /// Monogram: 1 px rounded-square outline radius (R§6.4).
    pub const MONOGRAM_RADIUS: f32 = 2.5;
    /// Monogram letter size, one letter / two letters (R§6.4).
    pub const MONOGRAM_FONT: (f32, f32) = (8.0, 7.0);

    // -- Pace bar rows (R§6.6)
    /// Hover card bar width (R§8.1: inner width 296).
    pub const CARD_BAR_W: f32 = 296.0;
    /// Hover card bar row height (R§6.6).
    pub const CARD_BAR_ROW_H: f32 = 11.0;
    /// Flyout bar width (F§2: inner card width 310).
    pub const FLYOUT_BAR_W: f32 = 310.0;
    /// Flyout bar row height (R§6.6, F§4.4).
    pub const FLYOUT_BAR_ROW_H: f32 = 9.0;

    // -- Hover card (R§8)
    /// Hover card width (R§8.1).
    pub const CARD_W: f32 = 320.0;
    /// Hover card padding, all sides (R§8.1).
    pub const CARD_PAD: f32 = 12.0;
    /// Hover card: clamp inside the work area by 12 px (R§8.1).
    pub const CARD_EDGE_MARGIN: f32 = 12.0;
    /// Hover card: 8 px between the card and the taskbar (R§8.1).
    pub const CARD_TASKBAR_GAP: f32 = 8.0;
    /// "Also tracked" time column minimum width (R§8.2).
    pub const ALSO_TIME_COL_MIN: f32 = 56.0;

    // -- Flyout (F§1–F§6)
    /// Flyout width (F§1.2).
    pub const FLYOUT_W: f32 = 360.0;
    /// Flyout maximum height before the list scrolls: `min(560, work-area height − 24)` (F§1.2).
    pub const FLYOUT_MAX_H: f32 = 560.0;
    /// Flyout distance from the work-area edges (F§1.3).
    pub const FLYOUT_EDGE_MARGIN: f32 = 12.0;
    /// Sparkline box (F§6).
    pub const SPARK_W: f32 = 55.0;
    /// Sparkline box height (F§6).
    pub const SPARK_H: f32 = 16.0;
    /// Sparkline column width before scaling (F§6: `round(3 s)`).
    pub const SPARK_COL_W: f32 = 3.0;
}
