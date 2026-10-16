//! Live smoke test of Cursor's session reader against this machine's `state.vscdb`, with the
//! production file access (read-only, page by page). Ignored by default; run by hand with
//! `cargo test -p lastdrop-providers --test cursor_live -- --ignored --nocapture`.
//!
//! It never sends anything: it prints only whether a session was found, how long the read took,
//! and whether the session's expiry has passed — never the token, the user id or any other value
//! of the database.
#![cfg(windows)]
#![expect(
    clippy::print_stdout,
    reason = "a manual smoke test reports what it found"
)]

use std::time::Instant;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::ProviderId;
use lastdrop_core::settings::AccountSettings;
use lastdrop_providers::cursor;
use lastdrop_providers::fakes::{FakeHttp, FakeSecrets};
use lastdrop_providers::real::{RealClock, RealEnv, RealFs, RealSpawner};
use lastdrop_providers::{AccountSource, FetchContext, FetchError};

#[test]
#[ignore = "reads this machine's Cursor state.vscdb; run by hand"]
fn live_cursor_session() {
    let (clock, http, fs, secrets, spawner, env) = (
        RealClock,
        FakeHttp::new(),
        RealFs,
        FakeSecrets::new(),
        RealSpawner,
        RealEnv,
    );
    let account = AccountSource::resolve(ProviderId::Cursor, &AccountSettings::primary(), &env)
        .expect("a Cursor source");
    let AccountSource::Cursor { state_db, .. } = &account else {
        panic!("not a Cursor source");
    };
    let ctx = FetchContext {
        clock: &clock,
        http: &http,
        fs: &fs,
        secrets: &secrets,
        spawner: &spawner,
        env: &env,
        deadline: Timestamp::now() + SignedDuration::from_secs(20),
        manual: true,
    };
    let started = Instant::now();
    let token = cursor::read_token(&ctx, state_db);
    println!("read took {} ms", started.elapsed().as_millis());
    match token {
        Ok(token) => {
            println!("session found ({} characters)", token.expose().len());
            match cursor::session(&token, Timestamp::now()) {
                Ok(_) => println!("the session is current"),
                Err(FetchError::Auth { .. }) => println!("the session has expired"),
                Err(e) => println!("the session is not usable: {e}"),
            }
        }
        Err(e) => println!("no session: {e}"),
    }
}
