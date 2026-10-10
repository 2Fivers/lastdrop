//! Ranking: auto rows and the winner (R§3.10).

use std::cmp::Ordering;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::consts::{AUTO_ROWS, AUTO_SWAP_INTERVAL, AUTO_SWAP_MARGIN};
use crate::model::MeterKey;

/// `Some` values first in ascending order, `None` last.
fn none_last<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Floats compared with NaN treated as the smallest value.
fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b)
        .unwrap_or_else(|| a.is_nan().cmp(&b.is_nan()).reverse())
}

/// The R§3.10 tie break: service order, then meter order.
fn tie_break(a: &Candidate, b: &Candidate) -> Ordering {
    a.service_order
        .cmp(&b.service_order)
        .then(a.meter_order.cmp(&b.meter_order))
}

/// Candidate eligibility (R§3.10): balance meters only when WARNING or worse.
fn eligible(c: &Candidate) -> bool {
    !c.is_balance || c.class >= DisplayClass::Warning
}

/// Display class, the first rank key (R§3.10): EXHAUSTED 5 > CRITICAL 4 > WARNING 3 > STALE 2 >
/// OK 1 > LOADING 0. Ordered so that `>` means "more constrained".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DisplayClass {
    /// No reading yet.
    Loading = 0,
    /// Fresh and OK (IDLE and JUST_RESET included).
    Ok = 1,
    /// The service is stale.
    Stale = 2,
    /// WARNING.
    Warning = 3,
    /// CRITICAL (level or runway).
    Critical = 4,
    /// EXHAUSTED.
    Exhausted = 5,
}

/// One rankable meter (R§3.10 "Candidates: every meter of every enabled service").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// The meter.
    pub key: MeterKey,
    /// Display class.
    pub class: DisplayClass,
    /// A balance meter: a candidate only when WARNING or worse, otherwise a filler (R§3.10
    /// "Too few candidates").
    pub is_balance: bool,
    /// `R` (EXHAUSTED: later first; `None` counts as latest. CRITICAL by level without `t_out`:
    /// earlier first; `None` last).
    pub resets_at: Option<Timestamp>,
    /// `t_out` (CRITICAL and WARNING: earlier first).
    pub t_out: Option<Timestamp>,
    /// Pressure `q = max(u, p)`; `p = u` without a valid forecast; IDLE `q = 0` (OK and STALE:
    /// higher first).
    pub pressure: f64,
    /// Balance `days_left` for the filler rule (fewest first; none last).
    pub days_left: Option<f64>,
    /// Service order from settings (tie break 1).
    pub service_order: usize,
    /// Meter order within the service (tie break 2).
    pub meter_order: u16,
}

/// Rank comparison, most constrained first (`Less` = `a` ranks before `b`), R§3.10:
///
/// 1. display class, higher first;
/// 2. within EXHAUSTED: later `R` first (`None` = latest); within CRITICAL and WARNING: earlier
///    `t_out` first (CRITICAL by level without `t_out`: use `R`; `R` none: last); within OK and
///    STALE: higher `q` first;
/// 3. ties: service order, then meter order.
pub fn compare(a: &Candidate, b: &Candidate) -> Ordering {
    let by_class = b.class.cmp(&a.class);
    let within = match a.class {
        // Out for longer first: later `R` first, `None` counts as latest.
        DisplayClass::Exhausted => none_last(a.resets_at, b.resets_at).reverse(),
        // Runs out sooner first (critical by level without `t_out`: its `R`; neither: last).
        DisplayClass::Critical | DisplayClass::Warning => {
            none_last(a.t_out.or(a.resets_at), b.t_out.or(b.resets_at))
        }
        DisplayClass::Ok | DisplayClass::Stale => cmp_f64(b.pressure, a.pressure),
        DisplayClass::Loading => Ordering::Equal,
    };
    by_class.then(within).then_with(|| tie_break(a, b))
}

/// Indices of `candidates` in rank order, applying candidate eligibility (balances only when
/// WARNING or worse) and the "too few candidates" rule (empty places go to OK balance meters,
/// fewest `days_left` first, no forecast last). LOADING meters rank last.
pub fn rank(candidates: &[Candidate]) -> Vec<usize> {
    let mut ranked: Vec<usize> = (0..candidates.len())
        .filter(|&i| eligible(&candidates[i]))
        .collect();
    ranked.sort_by(|&a, &b| compare(&candidates[a], &candidates[b]));
    if ranked.len() < AUTO_ROWS {
        // Balances that are not candidates (OK, stale) fill the empty places, fewest days left
        // first; a balance still loading has nothing to show and only comes after them.
        let mut fillers: Vec<usize> = (0..candidates.len())
            .filter(|&i| !eligible(&candidates[i]))
            .collect();
        fillers.sort_by(|&a, &b| {
            let (a, b) = (&candidates[a], &candidates[b]);
            (a.class == DisplayClass::Loading)
                .cmp(&(b.class == DisplayClass::Loading))
                .then_with(|| match (a.days_left, b.days_left) {
                    (Some(x), Some(y)) => cmp_f64(x, y),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                })
                .then_with(|| tie_break(a, b))
        });
        let missing = AUTO_ROWS - ranked.len();
        ranked.extend(fillers.into_iter().take(missing));
    }
    ranked
}

/// The winner (tray icon) = rank 1 over all candidates, regardless of what the readout shows
/// (R§3.10). `None` when there is no meter at all (the tray shows the drop).
pub fn winner(candidates: &[Candidate]) -> Option<usize> {
    rank(candidates).first().copied()
}

/// Auto-row state carried between evaluations (R§3.10 "Auto hysteresis").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoRows {
    /// The meters shown, in display order (service/meter order, not rank order).
    pub shown: Vec<MeterKey>,
    /// When the last same-class swap happened (at most one per `AUTO_SWAP_INTERVAL`).
    pub last_same_class_swap: Option<Timestamp>,
}

/// Auto rows (R§3.10): the top `n` (2, or 1 in the compact form) **displayed in service/meter
/// order**, with hysteresis: a shown row is replaced only when the challenger has a higher
/// display class, or the same class and `q ≥ q_shown + AUTO_SWAP_MARGIN`; same-class swaps
/// happen at most once per `AUTO_SWAP_INTERVAL`. Returns the new state.
pub fn select_rows(
    candidates: &[Candidate],
    n: usize,
    now: Timestamp,
    prev: &AutoRows,
) -> AutoRows {
    let ranked = rank(candidates);
    // Position in rank order of each ranked candidate.
    let position = |key: &MeterKey| ranked.iter().position(|&i| &candidates[i].key == key);
    // Keep the rows still ranked, best first; a smaller N keeps the best of them.
    let mut shown: Vec<usize> = prev.shown.iter().filter_map(position).collect();
    shown.sort_unstable();
    shown.dedup();
    shown.truncate(n);
    // Fill empty places from the top of the ranking.
    for p in 0..ranked.len() {
        if shown.len() >= n {
            break;
        }
        if !shown.contains(&p) {
            shown.push(p);
        }
    }
    shown.sort_unstable();

    let mut last_swap = prev.last_same_class_swap;
    // Challengers in rank order against the weakest shown row (the last in rank order).
    for p in 0..ranked.len() {
        if shown.contains(&p) {
            continue;
        }
        let Some(&weakest) = shown.last() else {
            break;
        };
        if p > weakest {
            // Everything shown ranks above every remaining challenger.
            break;
        }
        let (challenger, held) = (&candidates[ranked[p]], &candidates[ranked[weakest]]);
        let higher_class = challenger.class > held.class;
        let same_class = challenger.class == held.class
            && challenger.pressure >= held.pressure + AUTO_SWAP_MARGIN
            && last_swap.is_none_or(|t| now.duration_since(t) >= AUTO_SWAP_INTERVAL);
        if higher_class || same_class {
            if !higher_class {
                last_swap = Some(now);
            }
            shown.pop();
            shown.push(p);
            shown.sort_unstable();
        }
    }

    let mut keys: Vec<&Candidate> = shown.iter().map(|&p| &candidates[ranked[p]]).collect();
    keys.sort_by(|a, b| tie_break(a, b));
    AutoRows {
        shown: keys.into_iter().map(|c| c.key.clone()).collect(),
        last_same_class_swap: last_swap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountKey, MeterId, ProviderId};

    fn ts(mins: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + mins * 60).expect("valid")
    }

    fn cand(service: usize, meter: u16, class: DisplayClass, pressure: f64) -> Candidate {
        let provider = ProviderId::ALL[service % ProviderId::ALL.len()];
        Candidate {
            key: MeterKey {
                provider,
                account: AccountKey::primary(),
                meter: MeterId(format!("m{meter}")),
            },
            class,
            is_balance: false,
            resets_at: None,
            t_out: None,
            pressure,
            days_left: None,
            service_order: service,
            meter_order: meter,
        }
    }

    #[test]
    fn class_then_key_then_order() {
        let mut ex_late = cand(0, 0, DisplayClass::Exhausted, 1.0);
        ex_late.resets_at = Some(ts(100));
        let mut ex_early = cand(0, 1, DisplayClass::Exhausted, 1.0);
        ex_early.resets_at = Some(ts(50));
        let ex_none = cand(0, 2, DisplayClass::Exhausted, 1.0);
        assert_eq!(compare(&ex_none, &ex_late), Ordering::Less);
        assert_eq!(compare(&ex_late, &ex_early), Ordering::Less);

        let mut crit_soon = cand(1, 0, DisplayClass::Critical, 0.9);
        crit_soon.t_out = Some(ts(10));
        let mut crit_level = cand(0, 0, DisplayClass::Critical, 0.95);
        crit_level.resets_at = Some(ts(30));
        let crit_none = cand(0, 1, DisplayClass::Critical, 0.99);
        assert_eq!(compare(&crit_soon, &crit_level), Ordering::Less);
        assert_eq!(compare(&crit_level, &crit_none), Ordering::Less);
        assert_eq!(compare(&ex_early, &crit_soon), Ordering::Less);

        let ok_hi = cand(1, 0, DisplayClass::Ok, 0.8);
        let ok_lo = cand(0, 0, DisplayClass::Ok, 0.2);
        let stale = cand(2, 0, DisplayClass::Stale, 0.1);
        assert_eq!(compare(&ok_hi, &ok_lo), Ordering::Less);
        assert_eq!(compare(&stale, &ok_hi), Ordering::Less);
        let tie_a = cand(0, 1, DisplayClass::Ok, 0.5);
        let tie_b = cand(1, 0, DisplayClass::Ok, 0.5);
        assert_eq!(compare(&tie_a, &tie_b), Ordering::Less);
    }

    #[test]
    fn balances_are_candidates_only_in_trouble_and_fill_empty_places() {
        let window = cand(0, 0, DisplayClass::Ok, 0.3);
        let mut bal_ok = cand(2, 0, DisplayClass::Ok, 0.6);
        bal_ok.is_balance = true;
        bal_ok.days_left = Some(9.0);
        let mut bal_ok2 = cand(2, 1, DisplayClass::Ok, 0.1);
        bal_ok2.is_balance = true;
        bal_ok2.days_left = Some(30.0);
        // One window meter in total: the best OK balance fills the second place.
        assert_eq!(
            rank(&[window.clone(), bal_ok2.clone(), bal_ok.clone()]),
            [0, 2]
        );
        // A warning balance is a candidate on its own.
        let mut bal_warn = bal_ok.clone();
        bal_warn.class = DisplayClass::Warning;
        let window2 = cand(1, 0, DisplayClass::Ok, 0.9);
        assert_eq!(
            rank(&[window.clone(), window2.clone(), bal_warn]),
            [2, 1, 0]
        );
        // Enough window meters: OK balances are left out.
        assert_eq!(rank(&[window, window2, bal_ok]), [1, 0]);
        assert_eq!(winner(&[]), None);
    }

    #[test]
    fn auto_rows_are_in_display_order_with_hysteresis() {
        let a = cand(0, 0, DisplayClass::Ok, 0.50);
        let b = cand(0, 1, DisplayClass::Ok, 0.40);
        let c = cand(1, 0, DisplayClass::Ok, 0.30);
        let first = select_rows(
            &[a.clone(), b.clone(), c.clone()],
            2,
            ts(0),
            &AutoRows::default(),
        );
        assert_eq!(first.shown, [a.key.clone(), b.key.clone()]);
        // c passes b, but by less than the margin: no swap.
        let c2 = cand(1, 0, DisplayClass::Ok, 0.44);
        let same = select_rows(&[a.clone(), b.clone(), c2], 2, ts(1), &first);
        assert_eq!(same.shown, first.shown);
        // By the margin: swapped, shown in service/meter order.
        let c3 = cand(1, 0, DisplayClass::Ok, 0.46);
        let swapped = select_rows(&[a.clone(), b.clone(), c3], 2, ts(2), &first);
        assert_eq!(swapped.shown, [a.key.clone(), c.key.clone()]);
        assert_eq!(swapped.last_same_class_swap, Some(ts(2)));
        // b climbs back within the swap interval: held.
        let b2 = cand(0, 1, DisplayClass::Ok, 0.60);
        let c4 = cand(1, 0, DisplayClass::Ok, 0.46);
        let held = select_rows(&[a.clone(), b2.clone(), c4.clone()], 2, ts(5), &swapped);
        assert_eq!(held.shown, swapped.shown);
        // A higher class swaps at once.
        let b_warn = cand(0, 1, DisplayClass::Warning, 0.60);
        let up = select_rows(&[a.clone(), b_warn, c4.clone()], 2, ts(6), &held);
        assert_eq!(up.shown, [a.key.clone(), b.key.clone()]);
        // After the interval a same-class swap is allowed again.
        let later = select_rows(&[a.clone(), b2, c4], 2, ts(13), &held);
        assert_eq!(later.shown, [a.key.clone(), b.key.clone()]);
        // N = 1 keeps the best shown row; vanished rows are replaced.
        let one = select_rows(&[a.clone(), b.clone()], 1, ts(20), &first);
        assert_eq!(one.shown, std::slice::from_ref(&a.key));
        let gone = select_rows(std::slice::from_ref(&c), 2, ts(21), &first);
        assert_eq!(gone.shown, [c.key]);
    }
}
