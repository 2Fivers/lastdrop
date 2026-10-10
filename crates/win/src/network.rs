//! Internet connectivity from Network List Manager, to tell OFFLINE from NO_DATA (R§3.8:
//! "OFFLINE (no internet per Network List Manager, or DNS/connect failures)").
//!
//! Threading: refresh workers (COM initialized as MTA on the calling thread).

/// `INetworkListManager::IsConnectedToInternet`; `None` when COM or NLM fails (then the fetch's
/// own error decides).
pub fn connected_to_internet() -> Option<bool> {
    todo!("M1: Network List Manager")
}
