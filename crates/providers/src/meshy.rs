//! Meshy: a credit balance (R§2 `Cr` / `credits` / `Credits`, R§3.6).
//!
//! Source (ARCHITECTURE provider table): `GET https://api.meshy.ai/openapi/v1/balance` with the
//! API key from Credential Manager (`Lastdrop:meshy:<account>:api_key`), stored there by the
//! settings window. The key is a secret header and never logged.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{AccountRef, Balance, ProviderId, Snapshot};
use lastdrop_core::secret::SecretString;

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::{HttpRequest, HttpResponse};
use crate::{FetchContext, Provider};

/// The documented balance endpoint (GET).
pub const BALANCE_URL: &str = "https://api.meshy.ai/openapi/v1/balance";

/// The credential name of the API key within the account.
pub const API_KEY_NAME: &str = "api_key";

/// Timeout of the balance request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Lower bound between fetches: 5 minutes (**not from the spec**: a documented endpoint, but a
/// balance changes slowly and R§3.6 needs no finer history).
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The Meshy provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeshyProvider;

impl MeshyProvider {
    /// The provider.
    pub fn new() -> MeshyProvider {
        MeshyProvider
    }
}

impl Provider for MeshyProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Meshy
    }

    /// Key from the secret store (missing → `FetchError::Auth`) → request → interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        todo!("M1: Meshy balance")
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }
}

/// `GET BALANCE_URL` with `Authorization: Bearer <key>` (a secret header).
pub fn request(key: &SecretString, timeout: Duration) -> HttpRequest {
    todo!("M1: balance request")
}

/// 200 → [`parse_balance`]; 401 → Auth; 429/403 → RateLimited; 5xx → Transient.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<f64, FetchError> {
    todo!("M1: map the HTTP status")
}

/// The `balance` field of the JSON body (credits); tolerant of unknown fields, an error when
/// missing (never 0).
pub fn parse_balance(body: &[u8]) -> Result<f64, FetchError> {
    todo!("M1: parse balance")
}

/// The credits balance meter (`credits`, unit credits, no reported top-up: R§3.6 detects top-ups
/// from history).
pub fn balance(amount: f64) -> Balance {
    todo!("M1: Meshy balance meter")
}

/// The snapshot of a balance reading.
pub fn to_snapshot(amount: f64, account: AccountRef, fetched_at: Timestamp) -> Snapshot {
    todo!("M1: Meshy snapshot")
}
