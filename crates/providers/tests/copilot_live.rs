//! Live smoke test of the Copilot provider with this machine's GitHub CLI sign-in and the
//! production I/O (`RealSpawner` runs `gh auth token` without a window, `UreqHttp` sends the
//! request). Ignored by default; run by hand with
//! `cargo test -p lastdrop-providers --test copilot_live -- --ignored --nocapture`.
//!
//! It prints only the plan, the meter ids, used shares, limited flags and the reset time, or the
//! error class — never the token, login, user id or anything else the answer holds.
#![cfg(windows)]
#![expect(
    clippy::print_stdout,
    reason = "a manual smoke test reports the values it read"
)]

use std::time::Instant;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::ProviderId;
use lastdrop_core::settings::AccountSettings;
use lastdrop_providers::copilot::CopilotProvider;
use lastdrop_providers::fakes::FakeSecrets;
use lastdrop_providers::real::{RealClock, RealEnv, RealFs, RealSpawner, UreqHttp};
use lastdrop_providers::{AccountSource, FetchContext, Provider};

#[test]
#[ignore = "runs gh and talks to api.github.com; run by hand"]
fn live_copilot_quota() {
    let (clock, http, fs, secrets, spawner, env) = (
        RealClock,
        UreqHttp::new(),
        RealFs,
        FakeSecrets::new(),
        RealSpawner,
        RealEnv,
    );
    let account = AccountSource::resolve(ProviderId::Copilot, &AccountSettings::primary(), &env)
        .expect("a Copilot source");
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
    let provider = CopilotProvider::new();
    println!(
        "credential fingerprint present: {}",
        provider.credential_fingerprint(&ctx, &account).is_some()
    );
    let started = Instant::now();
    let result = provider.fetch(&ctx, &account);
    println!("fetch took {} ms", started.elapsed().as_millis());
    match result {
        Ok(snap) => {
            println!("plan: {:?}", snap.account.plan);
            for m in &snap.meters {
                println!(
                    "{} ({}): used {:.3}, limited {}, resets {:?}",
                    m.id.0, m.names.short_label, m.used, m.limited, m.resets_at
                );
            }
        }
        Err(e) => panic!("fetch failed: {e:?}"),
    }
}
