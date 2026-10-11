//! Live smoke test of `UreqHttp`'s TLS: one GET without credentials to Meshy's documented balance
//! endpoint, which proves the SChannel connector and the Windows root store work end to end.
//! Ignored by default; run by hand with
//! `cargo test -p lastdrop-providers --test tls_live -- --ignored --nocapture`.
//!
//! It sends no key, cookie or other header of the owner's, and prints only the status code and
//! how long the request took.
#![cfg(windows)]
#![expect(
    clippy::print_stdout,
    reason = "a manual smoke test reports the values it read"
)]

use std::time::{Duration, Instant};

use lastdrop_providers::io::{Http, HttpRequest, Method};
use lastdrop_providers::real::UreqHttp;

#[test]
#[ignore = "talks to api.meshy.ai; run by hand"]
fn live_tls_handshake() {
    let request = HttpRequest {
        method: Method::Get,
        url: "https://api.meshy.ai/openapi/v1/balance".to_owned(),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(15),
    };
    let started = Instant::now();
    let response = UreqHttp::new()
        .request(&request)
        .expect("the TLS handshake and the request succeed");
    println!(
        "status {} in {} ms",
        response.status,
        started.elapsed().as_millis()
    );
    // Without a key the endpoint refuses; any HTTP answer means TLS worked.
    assert!(
        (400..500).contains(&response.status),
        "unexpected status {}",
        response.status
    );
}
