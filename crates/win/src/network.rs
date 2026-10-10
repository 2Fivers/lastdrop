//! Internet connectivity from Network List Manager, to tell OFFLINE from NO_DATA (R§3.8:
//! "OFFLINE (no internet per Network List Manager, or DNS/connect failures)").
//!
//! Network List Manager answers from the local NLA service's cached state (its own periodic
//! probe); asking it sends nothing over the network.
//!
//! Threading: refresh workers (COM initialized as MTA on the calling thread).

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Networking::NetworkListManager::{INetworkListManager, NetworkListManager};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};

/// `INetworkListManager::IsConnectedToInternet`; `None` when COM or NLM fails (then the fetch's
/// own error decides).
///
/// Works on a thread without COM too: it then joins the MTA for the duration of the call.
pub fn connected_to_internet() -> Option<bool> {
    let _com = ComScope::enter();
    // SAFETY: COM is initialized on this thread (by the caller or by `_com`, which outlives the
    // interface: `nlm` is declared after it and dropped first).
    let nlm: INetworkListManager =
        match unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL) } {
            Ok(nlm) => nlm,
            Err(e) => {
                log::warn!("network: Network List Manager unavailable: {e}");
                return None;
            }
        };
    // SAFETY: a plain call on a live interface.
    match unsafe { nlm.IsConnectedToInternet() } {
        Ok(connected) => Some(connected.as_bool()),
        Err(e) => {
            log::warn!("network: IsConnectedToInternet failed: {e}");
            None
        }
    }
}

/// Joins the MTA when the thread has no COM yet, and leaves it on drop. A thread that already
/// has COM (MTA or STA) keeps it untouched.
struct ComScope {
    initialized: bool,
}

impl ComScope {
    fn enter() -> ComScope {
        // SAFETY: plain COM initialization of the calling thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_OK and S_FALSE both need a matching CoUninitialize; RPC_E_CHANGED_MODE (an STA
        // thread) does not, and COM is usable there anyway.
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            log::debug!("network: CoInitializeEx failed: {hr:?}");
        }
        ComScope {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: balances the successful CoInitializeEx of `enter` on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_list_manager_answers() {
        // The answer depends on the machine; that NLM answers at all is what is tested.
        assert!(connected_to_internet().is_some());
        // A second call on the same thread (COM entered and left again) works too.
        assert!(connected_to_internet().is_some());
    }

    #[test]
    fn works_on_a_thread_that_already_has_an_sta() {
        std::thread::spawn(|| {
            // SAFETY: test thread joins an STA for its lifetime.
            let hr = unsafe {
                CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED)
            };
            assert!(hr.is_ok());
            assert!(connected_to_internet().is_some());
            // SAFETY: balances the CoInitializeEx above.
            unsafe { CoUninitialize() };
        })
        .join()
        .expect("thread");
    }
}
