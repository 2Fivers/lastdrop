//! Live smoke test of the Claude Desktop cache source against this machine's Claude Desktop.
//! Ignored by default; run by hand with
//! `cargo test -p lastdrop-providers --test desktop_cache_live -- --ignored --nocapture`.
//!
//! It prints only whether Claude Code names its organisation (yes or no), how many usage responses
//! there are of any organisation and of that one, whether one was found, its content coding, its
//! age, how long the read took, and per meter id the used percentage and reset time — plus, for
//! comparison, the time and percentages of Desktop's newest history sample and which source the
//! provider (without the OAuth source) picks. Never a path, cache key, header, organisation id or
//! any other cache content: the cache holds the owner's conversations.
#![cfg(windows)]
#![expect(
    clippy::print_stdout,
    reason = "a manual smoke test reports the values it read"
)]

use std::time::Instant;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountRef, ProviderId, Snapshot, SourceKind};
use lastdrop_core::settings::{AccountSettings, ClaudeOptions};
use lastdrop_providers::claude::{self, ClaudeProvider, desktop, desktop_cache};
use lastdrop_providers::fakes::{FakeSecrets, FakeSpawner};
use lastdrop_providers::real::{RealClock, RealEnv, RealFs, UreqHttp};
use lastdrop_providers::{AccountSource, FetchContext, Provider};

fn meters(snapshot: &Snapshot) {
    for m in &snapshot.meters {
        println!(
            "  {}: used {:.1} %, resets_at {}",
            m.id.0,
            m.used * 100.0,
            m.resets_at
                .map_or_else(|| "(none)".to_owned(), |t| t.to_string())
        );
    }
}

fn age(at: Timestamp) -> String {
    let secs = Timestamp::now().duration_since(at).as_secs();
    format!("{} min {} s", secs / 60, secs % 60)
}

#[test]
#[ignore = "live: reads this machine's Claude Desktop HTTP cache (manual smoke test)"]
fn live_desktop_cache_smoke() {
    let env = RealEnv;
    let (clock, http, fs, secrets, spawner) = (
        RealClock,
        UreqHttp::new(),
        RealFs,
        FakeSecrets::new(),
        FakeSpawner::new(),
    );
    let ctx = FetchContext {
        clock: &clock,
        http: &http,
        fs: &fs,
        secrets: &secrets,
        spawner: &spawner,
        env: &env,
        deadline: Timestamp::now() + SignedDuration::from_secs(20),
        manual: false,
    };
    let account = AccountSource::resolve(ProviderId::Claude, &AccountSettings::primary(), &env)
        .expect("a Claude account source");
    let AccountSource::Claude {
        config_dir,
        state_file,
        desktop_dir: Some(desktop_dir),
        ..
    } = &account
    else {
        panic!("no Claude Desktop folder on this machine");
    };

    // Whether Claude Code names its organisation: yes or no only, never the id.
    let organisation = claude::organisation(&ctx, config_dir, state_file);
    println!(
        "Claude Code's organisation known: {}",
        if organisation.is_some() { "yes" } else { "no" }
    );
    let any = desktop_cache::read_detailed(&ctx, desktop_dir, None);
    match &any {
        Ok(c) => println!("usage responses of any organisation: {}", c.responses),
        Err(e) => println!("usage responses of any organisation: none ({})", e.detail()),
    }

    let started = Instant::now();
    let cached = desktop_cache::read_detailed(&ctx, desktop_dir, organisation.as_ref());
    let took = started.elapsed();
    match &cached {
        Ok(c) => {
            println!("entry found: yes");
            println!("content-encoding: {}", c.content_encoding);
            println!("zstd on disk: {}", if c.disk_zstd { "yes" } else { "no" });
            println!(
                "usage responses: {}, entries visited: {}",
                c.responses, c.visited
            );
            let at = c.usage.measured_at.expect("a response time");
            println!("response time: {at} (age {})", age(at));
            let snap = claude::to_snapshot(
                &c.usage,
                SourceKind::ClaudeDesktopCache,
                AccountRef::primary(),
                Timestamp::now(),
            );
            meters(&snap);
        }
        Err(e) => println!("entry found: no ({})", e.detail()),
    }
    println!("read took {} ms", took.as_millis());

    match desktop::read(&ctx, desktop_dir) {
        Ok(history) => {
            let at = history.measured_at.expect("a sample time");
            println!("history newest sample: {at} (age {})", age(at));
            let snap = claude::to_snapshot(
                &history,
                SourceKind::ClaudeDesktopHistory,
                AccountRef::primary(),
                Timestamp::now(),
            );
            meters(&snap);
        }
        Err(e) => println!("history: {}", e.detail()),
    }

    let no_oauth = ClaudeProvider::new(ClaudeOptions {
        oauth_usage: false,
        ..ClaudeOptions::default()
    });
    match no_oauth.fetch(&ctx, &account) {
        Ok(snap) => {
            println!(
                "provider without OAuth: {:?}, measured {} (age {}), health {:?}",
                snap.source,
                snap.measured_at,
                age(snap.measured_at),
                snap.health
            );
            meters(&snap);
        }
        Err(e) => println!("provider without OAuth: {}", e.detail()),
    }
    cached.expect("Claude Desktop's cache has a usage response");
}
