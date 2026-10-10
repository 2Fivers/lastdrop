//! Readings over time: the input of forecasts (R§3.5), balance statistics (R§3.6) and the
//! sparkline (F§6).

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::MeterKey;

/// One stored reading of one meter. Window meters store `u` and `R`; balance meters store the
/// amount and no `R`. This is also the line format of the app's history files (JSONL).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// `t_r` of the reading (R§3.1): the snapshot's `measured_at`
    /// ([`crate::model::Snapshot::reading_at`]), so a cached reading is placed when its values
    /// were true and cannot fake a rate.
    pub t: Timestamp,
    /// Window meter: `u`, the used share (R§3.1). Balance meter: the amount.
    #[serde(rename = "v")]
    pub value: f64,
    /// Window meter: `resets_at` of the reading, which decides the window instance (R§3.1).
    #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

impl Sample {
    /// A window meter reading.
    pub const fn window(t: Timestamp, used: f64, resets_at: Option<Timestamp>) -> Sample {
        Sample {
            t,
            value: used,
            resets_at,
        }
    }

    /// A balance reading.
    pub const fn balance(t: Timestamp, amount: f64) -> Sample {
        Sample {
            t,
            value: amount,
            resets_at: None,
        }
    }
}

/// Read access to the history of every meter (R§3.1 `history`: "earlier readings of the same
/// meter, ascending").
///
/// The app's store keeps in memory at least what the statistics need — the current window
/// instance of every window meter (≤ 31 days), 30 days of every balance (R§3.6 `ref`), and 14
/// local days for the sparkline (F§6) — thinned to one sample per 5 minutes; older data stays on
/// disk only.
pub trait HistoryView {
    /// Samples of `key` with `t ≥ since`, ascending by `t`, including the latest reading.
    /// Empty when there are none.
    fn samples(&self, key: &MeterKey, since: Timestamp) -> &[Sample];
}

/// A history with no samples (first run, tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHistory;

impl HistoryView for NoHistory {
    fn samples(&self, _key: &MeterKey, _since: Timestamp) -> &[Sample] {
        &[]
    }
}

/// An in-memory history, mainly for tests: each vector must be sorted by `t`.
impl HistoryView for BTreeMap<MeterKey, Vec<Sample>> {
    fn samples(&self, key: &MeterKey, since: Timestamp) -> &[Sample] {
        match self.get(key) {
            Some(v) => &v[v.partition_point(|s| s.t < since)..],
            None => &[],
        }
    }
}
