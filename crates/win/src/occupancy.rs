//! The occupancy thread: a UI Automation client (MTA) that measures where the taskbar's app
//! buttons are, with connection and transaction timeouts, so the readout never covers them
//! (ARCHITECTURE "Threads", ROADMAP "Open questions").
//!
//! Requested by the readout thread after taskbar location changes (debounced); answers through
//! its sink. It does nothing else. When UI Automation fails or times out, the answer is
//! [`OccupancyResult::Unknown`] and placement floats the readout instead of guessing.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::thread::JoinHandle;
use std::time::Duration;

use crate::placement::AppButtonExtents;

/// `IUIAutomation6::put_ConnectionTimeout` (**not from the spec**).
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

/// `IUIAutomation6::put_TransactionTimeout` (**not from the spec**).
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(1);

/// The answer to one measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OccupancyResult {
    /// The buttons' extents.
    Measured(AppButtonExtents),
    /// UI Automation failed or timed out.
    Unknown,
}

/// Handle to the occupancy thread (owned by the readout thread).
pub struct OccupancyHandle {
    #[expect(dead_code, reason = "used once the handle is implemented (M1)")]
    thread: Option<JoinHandle<()>>,
}

impl OccupancyHandle {
    /// Starts the thread (COM MTA, `CUIAutomation8`, timeouts set). `sink` runs on the occupancy
    /// thread and must only post to the readout thread.
    pub fn spawn(
        sink: Box<dyn Fn(OccupancyResult) + Send + 'static>,
    ) -> std::io::Result<OccupancyHandle> {
        todo!("M1: spawn the UIA thread")
    }

    /// Requests a measurement of the taskbar `taskbar_hwnd` (`HWND` as `isize`); requests that
    /// arrive while one runs are coalesced into one more.
    pub fn request(&self, taskbar_hwnd: isize) {
        todo!("M1: queue a measurement")
    }

    /// Stops and joins the thread.
    pub fn shutdown(self) {
        todo!("M1: stop and join")
    }
}
